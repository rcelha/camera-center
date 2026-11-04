//! Webcam enumeration and capture.
//!
//! * macOS: AVFoundation directly (frames delivered as BGRA by the OS).
//! * Linux / Windows: nokhwa (V4L2 / Media Foundation).

use std::time::Duration;

use anyhow::Result;
use cc_core::Frame;

#[cfg(target_os = "macos")]
mod avfoundation;
#[cfg(not(target_os = "macos"))]
mod nokhwa_backend;

#[cfg(target_os = "macos")]
use avfoundation as backend;
#[cfg(not(target_os = "macos"))]
use nokhwa_backend as backend;

/// Device names containing this are our own virtual camera and are hidden
/// from the input list to avoid feedback loops.
pub const VIRTUAL_CAMERA_NAME: &str = "Camera Center";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Human readable name, used as the persisted identifier.
    pub name: String,
    /// Backend specific identifier used to open the device.
    pub id: String,
}

/// A capture format offered by a device, reduced to what selection needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatCandidate {
    pub width: u32,
    pub height: u32,
    pub max_fps: f64,
    /// Decode cost preference, lower is better.
    pub rank: u8,
}

/// Asks the OS for camera permission (macOS) and waits for the answer.
/// Returns `true` when access is granted.
pub fn request_permission(timeout: Duration) -> bool {
    backend::request_permission(timeout)
}

/// Lists capture devices, excluding our own virtual camera.
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    Ok(backend::list_devices()?
        .into_iter()
        .filter(|d| !d.name.contains(VIRTUAL_CAMERA_NAME))
        .collect())
}

/// Picks the best format: a smooth frame rate first (≥24 fps), then the
/// largest height within `max_height`, then frame rates close to 30, then the
/// cheapest pixel format. Returns an index into `formats`.
pub fn choose_format(formats: &[FormatCandidate], max_height: Option<u32>) -> Option<usize> {
    let cap = max_height.unwrap_or(u32::MAX);
    let indexed = formats.iter().enumerate();
    let within: Vec<_> = indexed.clone().filter(|(_, f)| f.height <= cap).collect();
    if within.is_empty() {
        // Nothing fits the cap: take the smallest and let the pipeline scale.
        return indexed.min_by_key(|(_, f)| f.height).map(|(i, _)| i);
    }
    within
        .into_iter()
        .max_by_key(|(_, f)| {
            let fps = f.max_fps.round() as u32;
            (
                fps >= 24,
                f.height,
                fps.min(30),
                std::cmp::Reverse(f.rank),
                std::cmp::Reverse(fps),
            )
        })
        .map(|(i, _)| i)
}

/// An open camera stream producing RGBA frames.
pub struct CaptureSession {
    inner: backend::Session,
    device: DeviceInfo,
    format: String,
}

impl CaptureSession {
    /// Opens `device_name` (or the first device if `None` / not found).
    pub fn open(device_name: Option<&str>, max_height: Option<u32>) -> Result<Self> {
        let devices = list_devices()?;
        let device = device_name
            .and_then(|n| devices.iter().find(|d| d.name == n))
            .or_else(|| {
                if let Some(n) = device_name {
                    tracing::warn!("camera {n:?} not found, using the first available");
                }
                devices.first()
            })
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no camera found"))?;
        let (inner, format) = backend::Session::open(&device, max_height)?;
        tracing::info!(device = %device.name, %format, "camera opened");
        Ok(Self {
            inner,
            device,
            format,
        })
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.device
    }

    /// Human readable description of the active format.
    pub fn format(&self) -> &str {
        &self.format
    }

    /// Blocks until the next frame is available and returns it as RGBA.
    pub fn next_frame(&mut self) -> Result<Frame> {
        self.inner.next_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(width: u32, height: u32, max_fps: f64, rank: u8) -> FormatCandidate {
        FormatCandidate {
            width,
            height,
            max_fps,
            rank,
        }
    }

    #[test]
    fn prefers_highest_resolution_within_cap() {
        let formats = [
            f(640, 480, 30.0, 0),
            f(1280, 720, 30.0, 0),
            f(1920, 1080, 30.0, 2),
        ];
        assert_eq!(choose_format(&formats, Some(720)), Some(1));
        assert_eq!(choose_format(&formats, None), Some(2));
    }

    #[test]
    fn prefers_smooth_frame_rate_over_resolution() {
        let formats = [f(1920, 1080, 5.0, 0), f(1280, 720, 30.0, 0)];
        assert_eq!(choose_format(&formats, None), Some(1));
    }

    #[test]
    fn prefers_30fps_and_cheaper_decode() {
        let formats = [
            f(1280, 720, 30.0, 2),
            f(1280, 720, 60.0, 0),
            f(1280, 720, 30.0, 0),
        ];
        assert_eq!(choose_format(&formats, None), Some(2));
    }

    #[test]
    fn falls_back_to_smallest_when_cap_too_low() {
        let formats = [f(1280, 720, 30.0, 0), f(1920, 1080, 30.0, 0)];
        assert_eq!(choose_format(&formats, Some(480)), Some(0));
    }

    #[test]
    fn empty_list_has_no_choice() {
        assert_eq!(choose_format(&[], None), None);
    }
}
