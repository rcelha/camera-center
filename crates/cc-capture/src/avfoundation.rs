//! AVFoundation capture. AVFoundation converts whatever the camera delivers
//! (NV12, YUY2, MJPEG, …) to BGRA for us; we swizzle to RGBA on copy.

use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use block2::RcBlock;
use cc_core::Frame;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDeviceFormat, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeContinuityCamera,
    AVCaptureDeviceTypeExternal, AVCaptureOutput, AVCaptureSession, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};
use tracing::warn;

use crate::{DeviceInfo, FormatCandidate, choose_format};

fn media_type_video() -> Result<&'static objc2_av_foundation::AVMediaType> {
    // SAFETY: reading an immutable framework constant.
    unsafe { AVMediaTypeVideo }.ok_or_else(|| anyhow!("AVMediaTypeVideo unavailable"))
}

pub fn request_permission(timeout: Duration) -> bool {
    let Ok(media) = media_type_video() else {
        return false;
    };
    // SAFETY: class method with a valid media type.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    match status {
        AVAuthorizationStatus::Authorized => true,
        AVAuthorizationStatus::NotDetermined => {
            let (tx, rx) = mpsc::channel();
            let block = RcBlock::new(move |granted: Bool| {
                let _ = tx.send(granted.as_bool());
            });
            // SAFETY: the block is retained by AVFoundation for the callback.
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &block) };
            rx.recv_timeout(timeout).unwrap_or(false)
        }
        _ => false,
    }
}

fn discover() -> Result<Retained<NSArray<AVCaptureDevice>>> {
    let media = media_type_video()?;
    // SAFETY: framework constants and a valid discovery query.
    unsafe {
        let types = NSArray::from_slice(&[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeContinuityCamera,
        ]);
        let session =
            AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                &types,
                Some(media),
                AVCaptureDevicePosition::Unspecified,
            );
        Ok(session.devices())
    }
}

pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    Ok(discover()?
        .iter()
        .map(|d| unsafe {
            // SAFETY: simple property getters.
            DeviceInfo {
                name: d.localizedName().to_string(),
                id: d.uniqueID().to_string(),
            }
        })
        .collect())
}

/// Lower is better: native YUV formats are cheapest to convert.
fn subtype_rank(subtype: u32) -> u8 {
    match &subtype.to_be_bytes() {
        b"420v" | b"420f" => 0,
        b"yuvs" | b"2vuy" => 1,
        b"dmb1" | b"jpeg" => 2,
        _ => 3,
    }
}

fn describe_subtype(subtype: u32) -> String {
    String::from_utf8_lossy(&subtype.to_be_bytes()).into_owned()
}

struct FormatInfo {
    format: Retained<AVCaptureDeviceFormat>,
    candidate: FormatCandidate,
    subtype: u32,
    /// Exact minimum frame duration of the best supported rate ≤ 30 fps.
    frame_duration: Option<CMTime>,
}

const TARGET_FPS: f64 = 30.0;

fn formats(device: &AVCaptureDevice) -> Vec<FormatInfo> {
    // SAFETY: property getters on a valid device/format.
    unsafe {
        device
            .formats()
            .iter()
            .map(|format| {
                let desc = format.formatDescription();
                let dims = CMVideoFormatDescriptionGetDimensions(&desc);
                let subtype = desc.media_sub_type();
                let ranges = format.videoSupportedFrameRateRanges();
                let max_fps = ranges.iter().map(|r| r.maxFrameRate()).fold(0.0, f64::max);
                // Prefer a range reaching ~30 fps: use its exact duration if
                // it tops out at ≤30, otherwise 1/30 (inside the range).
                let frame_duration = ranges
                    .iter()
                    .filter(|r| r.minFrameRate() <= TARGET_FPS + 0.5)
                    .max_by(|a, b| a.maxFrameRate().total_cmp(&b.maxFrameRate()))
                    .map(|r| {
                        if r.maxFrameRate() <= TARGET_FPS + 0.5 {
                            r.minFrameDuration()
                        } else {
                            CMTime {
                                value: 1,
                                timescale: TARGET_FPS as i32,
                                flags: objc2_core_media::CMTimeFlags::Valid,
                                epoch: 0,
                            }
                        }
                    });
                FormatInfo {
                    frame_duration,
                    candidate: FormatCandidate {
                        width: dims.width.max(0) as u32,
                        height: dims.height.max(0) as u32,
                        max_fps,
                        rank: subtype_rank(subtype),
                    },
                    format,
                    subtype,
                }
            })
            .collect()
    }
}

/// Latest-frame mailbox shared with the delegate.
#[derive(Default)]
struct FrameSlot {
    frame: Mutex<Option<Frame>>,
    ready: Condvar,
}

pub struct DelegateIvars {
    slot: Arc<FrameSlot>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and we don't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "CCCaptureDelegate"]
    #[ivars = DelegateIvars]
    struct CaptureDelegate;

    unsafe impl NSObjectProtocol for CaptureDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CaptureDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            if let Some(frame) = copy_frame(sample) {
                let slot = &self.ivars().slot;
                *slot.frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
                slot.ready.notify_one();
            }
        }
    }
);

impl CaptureDelegate {
    fn new(slot: Arc<FrameSlot>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { slot });
        // SAFETY: NSObject's init on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }
}

/// Copies a BGRA sample buffer into an RGBA frame.
fn copy_frame(sample: &CMSampleBuffer) -> Option<Frame> {
    // SAFETY: the pixel buffer is locked read-only while its memory is read,
    // and rows are read within bytes_per_row * height.
    unsafe {
        let pb = sample.image_buffer()?;
        if CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return None;
        }
        let (w, h) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
        let stride = CVPixelBufferGetBytesPerRow(&pb);
        let base = CVPixelBufferGetBaseAddress(&pb) as *const u8;
        let mut data = vec![0u8; w * h * 4];
        if !base.is_null() {
            for (y, dst) in data.chunks_exact_mut(w * 4).enumerate() {
                let row = std::slice::from_raw_parts(base.add(y * stride), w * 4);
                // BGRA → RGBA is the same swizzle as RGBA → BGRA.
                cc_core::convert::rgba_to_bgra(row, dst);
            }
        }
        CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
        Frame::new(w as u32, h as u32, data).ok()
    }
}

pub struct Session {
    session: Retained<AVCaptureSession>,
    _delegate: Retained<CaptureDelegate>,
    _queue: DispatchRetained<DispatchQueue>,
    slot: Arc<FrameSlot>,
    seq: u64,
}

// SAFETY: AVCaptureSession is documented as thread-safe for start/stop; the
// delegate and queue are only touched by AVFoundation after setup.
unsafe impl Send for Session {}

impl Session {
    pub fn open(info: &DeviceInfo, max_height: Option<u32>) -> Result<(Self, String)> {
        let device = discover()?
            .iter()
            .find(|d| unsafe { d.uniqueID().to_string() } == info.id)
            .ok_or_else(|| anyhow!("camera {:?} disappeared", info.name))?;
        let formats = formats(&device);
        let candidates: Vec<_> = formats.iter().map(|f| f.candidate).collect();
        let chosen = choose_format(&candidates, max_height)
            .map(|i| &formats[i])
            .ok_or_else(|| anyhow!("camera {:?} reports no formats", info.name))?;
        let fps = chosen.candidate.max_fps.min(TARGET_FPS);

        let slot = Arc::new(FrameSlot::default());
        let delegate = CaptureDelegate::new(slot.clone());
        let queue = DispatchQueue::new("camera-center.capture", DispatchQueueAttr::SERIAL);

        // SAFETY: standard AVFoundation session setup with valid objects.
        unsafe {
            let input =
                AVCaptureDeviceInput::deviceInputWithDevice_error(&device).map_err(|e| {
                    anyhow!("cannot open {:?}: {}", info.name, e.localizedDescription())
                })?;
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            if !session.canAddInput(&input) {
                bail!("cannot use {:?} as capture input", info.name);
            }
            session.addInput(&input);

            let output = AVCaptureVideoDataOutput::new();
            let key: &NSString =
                &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString);
            let value = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
            let settings: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_slices(&[key], &[value.as_ref() as &AnyObject]);
            output.setVideoSettings(Some(&settings));
            output.setAlwaysDiscardsLateVideoFrames(true);
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
            if !session.canAddOutput(&output) {
                bail!("cannot add video output");
            }
            session.addOutput(&output);

            session.commitConfiguration();
            // Setting the active format switches the session preset to
            // InputPriority. Keep the device locked until the session runs,
            // otherwise startRunning can reapply the default preset.
            let locked = match device.lockForConfiguration() {
                Ok(()) => {
                    // Some UVC drivers raise NSInvalidArgumentException for
                    // durations they don't like; never let that abort us.
                    let configured = objc2::exception::catch(AssertUnwindSafe(|| {
                        device.setActiveFormat(&chosen.format);
                        if let Some(duration) = chosen.frame_duration {
                            device.setActiveVideoMinFrameDuration(duration);
                        }
                    }));
                    if let Err(e) = configured {
                        warn!("camera rejected format settings: {e:?}");
                    }
                    true
                }
                Err(e) => {
                    warn!(
                        "cannot configure camera format: {}",
                        e.localizedDescription()
                    );
                    false
                }
            };
            if let Err(e) = objc2::exception::catch(AssertUnwindSafe(|| session.startRunning())) {
                bail!("camera {:?} failed to start: {e:?}", info.name);
            }
            if locked {
                device.unlockForConfiguration();
            }

            let description = format!(
                "{}x{} @ {fps:.0} fps ({})",
                chosen.candidate.width,
                chosen.candidate.height,
                describe_subtype(chosen.subtype)
            );
            Ok((
                Self {
                    session,
                    _delegate: delegate,
                    _queue: queue,
                    slot,
                    seq: 0,
                },
                description,
            ))
        }
    }

    pub fn next_frame(&mut self) -> Result<Frame> {
        let guard = self.slot.frame.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, timeout) = self
            .slot
            .ready
            .wait_timeout_while(guard, Duration::from_secs(5), |f| f.is_none())
            .unwrap_or_else(|e| e.into_inner());
        if timeout.timed_out() {
            bail!("camera delivered no frames for 5 seconds");
        }
        let frame = guard.take().expect("checked above");
        self.seq += 1;
        Ok(frame.with_seq(self.seq))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: stopping a running session is always valid.
        unsafe { self.session.stopRunning() };
    }
}
