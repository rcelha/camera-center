//! Windows 11 output through a Media Foundation virtual camera.
//!
//! The app registers a *session lifetime* virtual camera backed by the
//! `cc_mf_source.dll` COM media source (see `crates/cc-mf-source`). When a
//! video app opens the camera, the Frame Server loads the source, which
//! creates the shared frame mapping; we then write NV12 frames into it.
//!
//! Status: compile-checked only; needs validation on Windows 11.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use cc_core::{Frame, convert};
use cc_vcam_protocol::{FRAME_SIZE, MAPPING_NAME, MAPPING_SIZE, SOURCE_CLSID_STR, SharedFrame};
use tracing::{info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::KernelStreaming::KSCATEGORY_VIDEO_CAMERA;
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MF_VERSION, MFCreateVirtualCamera, MFSTARTUP_FULL, MFStartup,
    MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_Session,
    MFVirtualCameraType_SoftwareCameraSource,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Memory::{
    FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
    UnmapViewOfFile,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_READ, RegCloseKey, RegOpenKeyExW,
};
use windows::core::HSTRING;

use crate::{DEVICE_NAME, OUTPUT_HEIGHT, OUTPUT_WIDTH, SinkStatus, VirtualCamera, to_output_size};

const RETRY: Duration = Duration::from_secs(1);

const _: () = assert!(
    OUTPUT_WIDTH == cc_vcam_protocol::WIDTH && OUTPUT_HEIGHT == cc_vcam_protocol::HEIGHT,
    "virtual camera output size must match the shared-memory protocol"
);

fn source_registered() -> bool {
    let path = HSTRING::from(format!("CLSID\\{SOURCE_CLSID_STR}\\InprocServer32"));
    let mut key = HKEY::default();
    // SAFETY: valid predefined key, string and out pointer.
    let ok = unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, &path, None, KEY_READ, &mut key) }.is_ok();
    if ok {
        // SAFETY: key was opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
    }
    ok
}

struct View {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    frame: SharedFrame,
}

impl View {
    fn open() -> Option<View> {
        let name = HSTRING::from(MAPPING_NAME);
        // SAFETY: plain Win32 calls; the view is MAPPING_SIZE bytes.
        unsafe {
            let handle = OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, &name).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, MAPPING_SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            let frame = SharedFrame::new(view.Value.cast());
            if !frame.is_valid() {
                let _ = UnmapViewOfFile(view);
                let _ = CloseHandle(handle);
                return None;
            }
            Some(View {
                handle,
                view,
                frame,
            })
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: created in `open`.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

pub struct MfVirtualCamera {
    camera: IMFVirtualCamera,
    view: Option<View>,
    last_attempt: Option<Instant>,
    nv12: Vec<u8>,
    epoch: Instant,
    streaming: bool,
}

// SAFETY: the virtual camera is created in the multithreaded apartment, whose
// objects may be used from any MTA thread.
unsafe impl Send for MfVirtualCamera {}

impl MfVirtualCamera {
    pub fn open() -> std::result::Result<Self, SinkStatus> {
        // SAFETY: COM/MF initialisation for this (processing) thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|e| SinkStatus::Error(format!("MFStartup failed: {e}")))?;
        }
        if !source_registered() {
            return Err(SinkStatus::NotInstalled {
                hint: "Register the media source once from an elevated prompt: regsvr32 cc_mf_source.dll".into(),
            });
        }
        let camera = Self::create().map_err(|e| {
            SinkStatus::Error(format!("{e:#} (virtual cameras require Windows 11)"))
        })?;
        info!("Media Foundation virtual camera registered");
        Ok(Self {
            camera,
            view: None,
            last_attempt: None,
            nv12: vec![0; FRAME_SIZE],
            epoch: Instant::now(),
            streaming: false,
        })
    }

    fn create() -> Result<IMFVirtualCamera> {
        let name = HSTRING::from(DEVICE_NAME);
        let clsid = HSTRING::from(SOURCE_CLSID_STR);
        // SAFETY: valid strings and category slice.
        unsafe {
            let camera = MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &name,
                &clsid,
                Some(&[KSCATEGORY_VIDEO_CAMERA]),
            )
            .map_err(|e| anyhow!("MFCreateVirtualCamera failed: {e}"))?;
            camera
                .Start(None)
                .map_err(|e| anyhow!("starting the virtual camera failed: {e}"))?;
            Ok(camera)
        }
    }
}

impl VirtualCamera for MfVirtualCamera {
    fn name(&self) -> String {
        DEVICE_NAME.to_string()
    }

    fn status(&self) -> SinkStatus {
        if self.streaming {
            SinkStatus::Streaming
        } else {
            SinkStatus::Ready
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<()> {
        if self.view.is_none() && self.last_attempt.is_none_or(|t| t.elapsed() >= RETRY) {
            // The mapping exists only while a video app has the camera open.
            self.last_attempt = Some(Instant::now());
            self.view = View::open();
        }
        let Some(view) = &self.view else {
            self.streaming = false;
            return Ok(());
        };
        let rgba = to_output_size(frame)?;
        convert::rgba_to_nv12(&rgba, OUTPUT_WIDTH, OUTPUT_HEIGHT, &mut self.nv12)?;
        let ts = (self.epoch.elapsed().as_nanos() / 100) as u64;
        if !view.frame.write(&self.nv12, ts) {
            warn!("shared frame size mismatch");
        }
        self.streaming = true;
        Ok(())
    }
}

impl Drop for MfVirtualCamera {
    fn drop(&mut self) {
        // SAFETY: plain COM calls on a valid object.
        unsafe {
            let _ = self.camera.Stop();
            let _ = self.camera.Shutdown();
        }
    }
}
