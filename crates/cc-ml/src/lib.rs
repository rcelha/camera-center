//! Person segmentation backed by ONNX Runtime.
//!
//! Models live in the repository's `models/` directory (Git LFS), described by
//! `models/models.toml`. See [`models_dir`] for how the directory is located
//! at runtime.

mod models;
mod smoothing;
mod u2net;

pub use models::{model_path, models_dir};
pub use smoothing::TemporalSmoother;
pub use u2net::{ExecutionBackend, U2Net};
