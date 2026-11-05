//! nokhwa based capture (V4L2 on Linux, Media Foundation on Windows).

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cc_core::Frame;
use nokhwa::Camera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
};
use tracing::warn;

use crate::{DeviceInfo, FormatCandidate, choose_format};

pub fn request_permission(_timeout: Duration) -> bool {
    true
}

pub fn list_devices() -> Result<Vec<DeviceInfo>> {
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
