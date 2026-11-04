//! macOS output: a CoreMediaIO Camera Extension (Swift, `platform/macos`)
//! publishes the "Camera Center" device. The app pushes BGRA frames into the
//! extension's *sink* stream; the extension republishes them on its source
//! stream, which is what video-call apps open.

mod extension;
mod ffi;

use std::ffi::c_void;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use cc_core::{Frame, convert};
use tracing::{info, warn};

pub use extension::{
    ActivationState, EXTENSION_BUNDLE_ID, INSTALL_HINT, activate, activation_state, install_blocker,
};
use ffi::*;

use crate::{
    DEVICE_NAME, OUTPUT_FPS, OUTPUT_HEIGHT, OUTPUT_WIDTH, SinkStatus, VirtualCamera, to_output_size,
};

/// Device UID published by the extension (must match CameraExtensionProvider.swift).
pub const DEVICE_UID: &str = "5B6A2F0C-7C1E-4D8B-9A51-3C0E2B7D4F61";

const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);

struct Connection {
    device: CMIOObjectID,
    sink: CMIOObjectID,
    queue: CMSimpleQueueRef,
    pool: CVPixelBufferPoolRef,
    format: CMFormatDescriptionRef,
}

// SAFETY: CoreMedia/CoreVideo objects are thread-safe reference counted
// objects; the connection is only used from one thread at a time.
unsafe impl Send for Connection {}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: all handles were obtained from the matching create/copy calls.
        unsafe {
            CMIODeviceStopStream(self.device, self.sink);
            if !self.format.is_null() {
                CFRelease(self.format as CFTypeRef);
            }
            if !self.pool.is_null() {
                CFRelease(self.pool as CFTypeRef);
            }
            if !self.queue.is_null() {
                CFRelease(self.queue as CFTypeRef);
            }
        }
    }
}

fn find_device() -> Option<CMIOObjectID> {
    object_ids(kCMIOObjectSystemObject, kCMIOHardwarePropertyDevices)
        .into_iter()
        .find(|&dev| {
            string_property(dev, kCMIODevicePropertyDeviceUID).as_deref() == Some(DEVICE_UID)
                || string_property(dev, kCMIOObjectPropertyName).as_deref() == Some(DEVICE_NAME)
        })
}

unsafe extern "C" fn queue_altered(
    _stream: CMIOObjectID,
    _token: *mut c_void,
    _refcon: *mut c_void,
) {
}

fn create_pool() -> Result<CVPixelBufferPoolRef> {
    // SAFETY: CoreFoundation objects are created and released in pairs below.
    unsafe {
        let fmt = kCVPixelFormatType_32BGRA as i32;
        let (w, h) = (OUTPUT_WIDTH as i32, OUTPUT_HEIGHT as i32);
        let n_fmt = CFNumberCreate(
            std::ptr::null(),
            kCFNumberSInt32Type,
            (&fmt as *const i32).cast(),
        );
        let n_w = CFNumberCreate(
            std::ptr::null(),
            kCFNumberSInt32Type,
            (&w as *const i32).cast(),
        );
        let n_h = CFNumberCreate(
            std::ptr::null(),
            kCFNumberSInt32Type,
            (&h as *const i32).cast(),
        );
        let empty = CFDictionaryCreate(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let keys = [
            kCVPixelBufferPixelFormatTypeKey,
            kCVPixelBufferWidthKey,
            kCVPixelBufferHeightKey,
            kCVPixelBufferIOSurfacePropertiesKey,
        ];
        let values = [n_fmt, n_w, n_h, empty];
        let attrs = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let mut pool: CVPixelBufferPoolRef = std::ptr::null_mut();
        let rc = CVPixelBufferPoolCreate(std::ptr::null(), std::ptr::null(), attrs, &mut pool);
        for v in values {
            CFRelease(v);
        }
        CFRelease(attrs);
        if rc != 0 || pool.is_null() {
            bail!("CVPixelBufferPoolCreate failed ({rc})");
        }
        Ok(pool)
    }
}

impl Connection {
    fn open() -> Result<Option<Self>> {
        let Some(device) = find_device() else {
            return Ok(None);
        };
        let streams = object_ids(device, kCMIODevicePropertyStreams);
        // The extension declares the source stream first and the sink second.
        let Some(&sink) = streams.get(1) else {
            bail!("camera extension device has no sink stream");
        };
        let mut queue: CMSimpleQueueRef = std::ptr::null_mut();
        // SAFETY: valid stream ID and out pointer.
        let rc = unsafe {
            CMIOStreamCopyBufferQueue(sink, Some(queue_altered), std::ptr::null_mut(), &mut queue)
        };
        if rc != 0 || queue.is_null() {
            bail!("CMIOStreamCopyBufferQueue failed ({rc})");
        }
        let mut conn = Connection {
            device,
            sink,
            queue,
            pool: std::ptr::null_mut(),
            format: std::ptr::null_mut(),
        };
        conn.pool = create_pool()?;
        // SAFETY: device and sink IDs come from CMIO.
        let rc = unsafe { CMIODeviceStartStream(device, sink) };
        if rc != 0 {
            bail!("CMIODeviceStartStream failed ({rc})");
        }
        info!(device, sink, "connected to camera extension sink");
        Ok(Some(conn))
    }

    fn enqueue(&mut self, rgba: &[u8], seq: u64) -> Result<()> {
        // SAFETY: queue is valid while the connection lives.
        let (count, capacity) = unsafe {
            (
                CMSimpleQueueGetCount(self.queue),
                CMSimpleQueueGetCapacity(self.queue),
            )
        };
        if count >= capacity {
            return Ok(()); // extension is behind, drop this frame
        }
        // SAFETY: standard CoreVideo/CoreMedia ownership rules are followed:
        // the pixel buffer is released after the sample buffer retains it and
        // the sample buffer's +1 reference is handed over to the queue.
        unsafe {
            let mut pb: CVPixelBufferRef = std::ptr::null_mut();
            let rc = CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), self.pool, &mut pb);
            if rc != 0 || pb.is_null() {
                bail!("CVPixelBufferPoolCreatePixelBuffer failed ({rc})");
            }
            CVPixelBufferLockBaseAddress(pb, 0);
            let base = CVPixelBufferGetBaseAddress(pb) as *mut u8;
            let stride = CVPixelBufferGetBytesPerRow(pb);
            let row = OUTPUT_WIDTH as usize * 4;
            for (y, src) in rgba.chunks_exact(row).enumerate() {
                let dst = std::slice::from_raw_parts_mut(base.add(y * stride), row);
                convert::rgba_to_bgra(src, dst);
            }
            CVPixelBufferUnlockBaseAddress(pb, 0);

            if self.format.is_null() {
                let rc = CMVideoFormatDescriptionCreateForImageBuffer(
                    std::ptr::null(),
                    pb,
                    &mut self.format,
                );
                if rc != 0 {
                    CFRelease(pb as CFTypeRef);
                    bail!("CMVideoFormatDescriptionCreateForImageBuffer failed ({rc})");
                }
            }
            let now = CMClockGetTime(CMClockGetHostTimeClock());
            let timing = CMSampleTimingInfo {
                duration: CMTime::new(1, OUTPUT_FPS as i32),
                presentationTimeStamp: now,
                decodeTimeStamp: CMTime::INVALID,
            };
            let mut sample: CMSampleBufferRef = std::ptr::null_mut();
            let rc = CMSampleBufferCreateForImageBuffer(
                std::ptr::null(),
                pb,
                1,
                std::ptr::null(),
                std::ptr::null_mut(),
                self.format,
                &timing,
                &mut sample,
            );
            CFRelease(pb as CFTypeRef);
            if rc != 0 || sample.is_null() {
                bail!("CMSampleBufferCreateForImageBuffer failed ({rc})");
            }
            let rc = CMSimpleQueueEnqueue(self.queue, sample as *const c_void);
            if rc != 0 {
                CFRelease(sample as CFTypeRef);
                bail!("CMSimpleQueueEnqueue failed ({rc}) for frame {seq}");
            }
        }
        Ok(())
    }
}

/// Client of the camera extension's sink stream.
pub struct CmioSink {
    conn: Option<Connection>,
    last_attempt: Option<Instant>,
    last_error: Option<String>,
    sent: u64,
}

impl CmioSink {
    pub fn new() -> Self {
        let mut sink = Self {
            conn: None,
            last_attempt: None,
            last_error: None,
            sent: 0,
        };
        sink.try_connect();
        sink
    }

    fn try_connect(&mut self) {
        if self.conn.is_some()
            || self
                .last_attempt
                .is_some_and(|t| t.elapsed() < RECONNECT_INTERVAL)
        {
            return;
        }
        self.last_attempt = Some(Instant::now());
        match Connection::open() {
            Ok(conn) => {
                self.conn = conn;
                self.last_error = None;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if self.last_error.as_deref() != Some(&msg) {
                    warn!("camera extension: {msg}");
                }
                self.last_error = Some(msg);
            }
        }
    }
}

impl Default for CmioSink {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualCamera for CmioSink {
    fn name(&self) -> String {
        DEVICE_NAME.to_string()
    }

    fn status(&self) -> SinkStatus {
        match (&self.conn, &self.last_error) {
            (Some(_), _) if self.sent > 0 => SinkStatus::Streaming,
            (Some(_), _) => SinkStatus::Ready,
            (None, Some(e)) => SinkStatus::Error(e.clone()),
            (None, None) => SinkStatus::NotInstalled {
                hint: match (activation_state(), install_blocker()) {
                    (ActivationState::Idle, Some(reason)) => reason,
                    (ActivationState::Idle, None) => {
                        "Click Install extension to add the Camera Center camera".into()
                    }
                    (other, _) => other.describe(),
                },
            },
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.try_connect();
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        let rgba = to_output_size(frame)?;
        if let Err(e) = conn.enqueue(&rgba, frame.seq) {
            // Drop the connection; we'll reconnect on a later frame.
            self.conn = None;
            self.sent = 0;
            return Err(e);
        }
        self.sent += 1;
        Ok(())
    }
}
