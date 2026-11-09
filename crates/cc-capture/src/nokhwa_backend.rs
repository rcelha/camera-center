//! nokhwa based capture (V4L2 on Linux, Media Foundation on Windows).

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cc_core::Frame;
use nokhwa::Camera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
use tracing::warn;

use crate::{DeviceInfo, FormatCandidate, choose_format};

pub fn request_permission(_timeout: Duration) -> bool {
    true
}

/// Enumerates V4L2 nodes directly: nokhwa's query also lists metadata and IR-only nodes, which share
/// the camera's name, so opening by name could pick a node with no usable format.
#[cfg(target_os = "linux")]
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    use v4l::capability::Flags;
    use v4l::video::Capture;

    let supported = [*b"YUYV", *b"NV12", *b"MJPG", *b"RGB3"];
    let mut nodes: Vec<(usize, String)> = v4l::context::enum_devices()
        .into_iter()
        .filter_map(|node| {
            let device = v4l::Device::with_path(node.path()).ok()?;
            let caps = device.query_caps().ok()?;
            if !caps.capabilities.contains(Flags::VIDEO_CAPTURE) {
                return None;
            }
            let formats = device.enum_formats().ok()?;
            if !formats.iter().any(|f| supported.contains(&f.fourcc.repr)) {
                return None;
            }
            Some((node.index(), node.name().unwrap_or(caps.card)))
        })
        .collect();
    nodes.sort_by_key(|(index, _)| *index);
    let names: Vec<String> = nodes.iter().map(|(_, n)| n.clone()).collect();
    Ok(nodes
        .into_iter()
        .map(|(index, name)| DeviceInfo {
            name: if names.iter().filter(|n| **n == name).count() > 1 {
                format!("{name} (/dev/video{index})")
            } else {
                name
            },
            id: index.to_string(),
        })
        .collect())
}

#[cfg(not(target_os = "linux"))]
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    use nokhwa::utils::ApiBackend;

    let devices = nokhwa::query(ApiBackend::Auto).context("failed to enumerate cameras")?;
    Ok(devices
        .into_iter()
        .map(|d| DeviceInfo {
            name: d.human_name(),
            id: match d.index() {
                CameraIndex::Index(i) => i.to_string(),
                CameraIndex::String(s) => s.clone(),
            },
        })
        .collect())
}

fn rank(f: FrameFormat) -> Option<u8> {
    match f {
        FrameFormat::YUYV => Some(0),
        FrameFormat::NV12 => Some(1),
        FrameFormat::MJPEG => Some(2),
        FrameFormat::RAWRGB => Some(3),
        _ => None,
    }
}

pub struct Session {
    camera: Camera,
    buffer: Vec<u8>,
    seq: u64,
}

impl Session {
    pub fn open(device: &DeviceInfo, max_height: Option<u32>) -> Result<(Self, String)> {
        let index = device
            .id
            .parse::<u32>()
            .map(CameraIndex::Index)
            .unwrap_or_else(|_| CameraIndex::String(device.id.clone()));
        let mut camera = Camera::new(
            index,
            RequestedFormat::new::<RgbAFormat>(RequestedFormatType::None),
        )
        .with_context(|| format!("failed to open camera {:?}", device.name))?;
        let formats: Vec<CameraFormat> = camera
            .compatible_camera_formats()
            .context("failed to list camera formats")?
            .into_iter()
            .filter(|f| rank(f.format()).is_some())
            .collect();
        let candidates: Vec<FormatCandidate> = formats
            .iter()
            .map(|f| FormatCandidate {
                width: f.resolution().width(),
                height: f.resolution().height(),
                max_fps: f.frame_rate() as f64,
                rank: rank(f.format()).unwrap_or(u8::MAX),
            })
            .collect();
        let chosen = choose_format(&candidates, max_height)
            .map(|i| formats[i])
            .ok_or_else(|| anyhow!("camera {:?} offers no supported format", device.name))?;
        camera
            .set_camera_requset(RequestedFormat::new::<RgbAFormat>(
                RequestedFormatType::Exact(chosen),
            ))
            .context("failed to configure camera format")?;
        camera.open_stream().context("failed to start camera")?;
        let description = camera.camera_format().to_string();
        Ok((
            Self {
                camera,
                buffer: Vec::new(),
                seq: 0,
            },
            description,
        ))
    }

    pub fn next_frame(&mut self) -> Result<Frame> {
        let buffer = self.camera.frame().context("failed to read camera frame")?;
        let res = buffer.resolution();
        let (w, h) = (res.width(), res.height());
        let len = w as usize * h as usize * 4;
        self.buffer.resize(len, 0);
        buffer
            .decode_image_to_buffer::<RgbAFormat>(&mut self.buffer)
            .context("failed to decode camera frame")?;
        if self.buffer.len() != len {
            bail!("decoded frame has unexpected size");
        }
        self.seq += 1;
        Ok(Frame::new(w, h, self.buffer.clone())?.with_seq(self.seq))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(e) = self.camera.stop_stream() {
            warn!("failed to stop camera: {e}");
        }
    }
}
