//! Person segmentation abstraction.

use anyhow::Result;

use crate::{Frame, Mask};

/// Produces a foreground mask for a frame.
///
/// Implementations may return a mask at any resolution (typically the model's
/// input size); consumers resize it to the frame they composite.
pub trait Segmenter: Send {
    /// Preferred input size. Callers can pre-scale frames to this size to
    /// avoid moving full-resolution frames between threads.
    fn input_size(&self) -> (u32, u32);

    fn segment(&mut self, frame: &Frame) -> Result<Mask>;
}
