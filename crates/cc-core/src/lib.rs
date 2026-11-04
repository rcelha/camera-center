//! Core building blocks for Camera Center.
//!
//! Everything in this crate is platform independent: frame and mask types,
//! the persisted configuration, pixel-format conversion, image effects and
//! the [`Pipeline`] that chains them together.
//!
//! All frames flowing through the pipeline are tightly packed RGBA8.

pub mod blur;
pub mod config;
pub mod convert;
pub mod effects;
pub mod frame;
pub mod pipeline;
pub mod resize;
pub mod segment;

pub use config::{AppConfig, EffectToggles, ProcessingResolution, SegmentationModel};
pub use frame::{Frame, Mask};
pub use pipeline::{Pipeline, Step, StepId};
pub use segment::Segmenter;
