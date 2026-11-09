//! Person segmentation backed by ONNX Runtime.
//!
//! Models live in the repository's `models/` directory (Git LFS), described by
//! `models/models.toml`. See [`models_dir`] for how the directory is located
//! at runtime.

mod models;
mod selfie;
mod session;
mod smoothing;
mod u2net;

use std::borrow::Cow;

use anyhow::Result;
use cc_core::{Frame, SegmentationModel, Segmenter, resize};

pub use models::{model_path, models_dir};
pub use selfie::SelfieSegmenter;
pub use session::ExecutionBackend;
pub use smoothing::TemporalSmoother;
pub use u2net::U2Net;

/// Loads the bundled ONNX file for `model` with the right pre/post-processing.
pub fn load(model: SegmentationModel, backend: ExecutionBackend) -> Result<Box<dyn Segmenter>> {
    let path = model_path(model)?;
    Ok(match model {
        SegmentationModel::SelfieSegmenter | SegmentationModel::SelfieSegmenterLandscape => {
            Box::new(SelfieSegmenter::load(&path, backend)?)
        }
        SegmentationModel::U2netHumanSeg | SegmentationModel::U2netp => {
            Box::new(U2Net::load(&path, backend)?)
        }
    })
}

/// The frame's RGBA pixels at `width`x`height`, borrowed if already that size.
fn scaled_rgba(frame: &Frame, width: u32, height: u32) -> Result<Cow<'_, [u8]>> {
    Ok(if (frame.width, frame.height) == (width, height) {
        Cow::Borrowed(&frame.data)
    } else {
        Cow::Owned(resize::rgba(
            &frame.data,
            frame.width,
            frame.height,
            width,
            height,
        )?)
    })
}
