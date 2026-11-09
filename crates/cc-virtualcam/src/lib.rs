//! Virtual camera outputs.
//!
//! | Platform | Backend |
//! |----------|---------|
//! | Linux    | v4l2loopback (`/dev/videoN`, YUYV) |
//! | macOS    | CoreMediaIO Camera Extension, fed through its sink stream (BGRA) |
//! | Windows  | Media Foundation virtual camera (`MFCreateVirtualCamera`), fed via shared memory (NV12) |
//!
//! Every backend outputs [`OUTPUT_WIDTH`]x[`OUTPUT_HEIGHT`]; frames of other
//! sizes are letterboxed.

use anyhow::Result;
use cc_core::Frame;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
mod null;
#[cfg(windows)]
mod windows;

pub use null::NullCamera;

/// Display name of the virtual device on every platform.
pub const DEVICE_NAME: &str = "Camera Center";
pub const OUTPUT_WIDTH: u32 = 1920;
pub const OUTPUT_HEIGHT: u32 = 1080;
pub const OUTPUT_FPS: u32 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkStatus {
    /// The platform component is missing; `hint` explains how to install it.
    NotInstalled {
        hint: String,
    },
    /// Installed but not receiving frames yet.
    Ready,
    /// Frames are being delivered.
    Streaming,
    Error(String),
}

impl SinkStatus {
    pub fn label(&self) -> &'static str {
        match self {
            SinkStatus::NotInstalled { .. } => "Not installed",
            SinkStatus::Ready => "Ready",
            SinkStatus::Streaming => "Streaming",
            SinkStatus::Error(_) => "Error",
        }
    }
}

/// A virtual camera device that other applications can open.
pub trait VirtualCamera: Send {
    /// Name other applications show for the device.
    fn name(&self) -> String;

    fn status(&self) -> SinkStatus;

    /// Delivers one frame. Implementations scale to their output size.
    fn send(&mut self, frame: &Frame) -> Result<()>;
}

/// Scales `frame` to the virtual camera output size (letterboxed).
pub fn to_output_size(frame: &Frame) -> Result<std::borrow::Cow<'_, [u8]>> {
    if (frame.width, frame.height) == (OUTPUT_WIDTH, OUTPUT_HEIGHT) {
        return Ok(std::borrow::Cow::Borrowed(&frame.data));
    }
    Ok(std::borrow::Cow::Owned(cc_core::resize::rgba_letterbox(
        &frame.data,
        frame.width,
        frame.height,
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT,
    )?))
}

/// Opens the platform virtual camera. Never fails: if the backend is
/// unavailable a [`NullCamera`] carrying the reason is returned so the rest of
/// the app keeps working.
pub fn open_platform_camera() -> Box<dyn VirtualCamera> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LoopbackSink::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::CmioSink::new())
    }
    #[cfg(windows)]
    {
        match windows::MfVirtualCamera::open() {
            Ok(cam) => Box::new(cam),
            Err(status) => Box::new(NullCamera::with_status(status)),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Box::new(NullCamera::with_status(SinkStatus::NotInstalled {
            hint: "virtual cameras are not supported on this platform".into(),
        }))
    }
}
