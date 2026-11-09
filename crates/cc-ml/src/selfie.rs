//! MediaPipe selfie segmenter (`selfie_segmenter`, `selfie_segmenter_landscape`).
//!
//! The ONNX files are converted from Google's TFLite models by
//! `models/convert_mediapipe.py`. Input is NHWC `[1, H, W, 3]` float RGB in
//! `[0, 1]`; output is `[1, H, W, 1]` person probability (already
//! sigmoid-activated), so it maps straight onto the mask without the min-max
//! stretch U²-Net needs. Stretching would turn an empty scene's noise into a
//! full-strength mask.

use std::path::Path;

use anyhow::{Result, ensure};
use cc_core::{Frame, Mask, Segmenter};
use ort::value::Tensor;

use crate::session::{ExecutionBackend, ModelSession};

pub struct SelfieSegmenter {
    model: ModelSession,
    width: u32,
    height: u32,
    tensor: Vec<f32>,
}

impl SelfieSegmenter {
    pub fn load(path: &Path, backend: ExecutionBackend) -> Result<Self> {
        let model = ModelSession::load(path, backend)?;
        let shape = model.input_shape.as_slice();
        ensure!(
            matches!(shape, [1, h, w, 3] if *h > 0 && *w > 0),
            "{} is not a selfie segmenter model: input shape {shape:?}",
            path.display()
        );
        let (height, width) = (shape[1] as u32, shape[2] as u32);
        Ok(Self {
            model,
            width,
            height,
            tensor: vec![0.0; (width * height * 3) as usize],
        })
    }
}

fn preprocess(rgba: &[u8], tensor: &mut [f32]) {
    for (dst, src) in tensor.chunks_exact_mut(3).zip(rgba.chunks_exact(4)) {
        for c in 0..3 {
            dst[c] = src[c] as f32 / 255.0;
        }
    }
}

fn postprocess(probs: &[f32], width: u32, height: u32) -> Result<Mask> {
    let data = probs
        .iter()
        .map(|&p| (p * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect();
    Mask::new(width, height, data)
}

impl Segmenter for SelfieSegmenter {
    fn input_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn segment(&mut self, frame: &Frame) -> Result<Mask> {
        let (w, h) = (self.width, self.height);
        let rgba = crate::scaled_rgba(frame, w, h)?;
        preprocess(&rgba, &mut self.tensor);
        let input =
            Tensor::<f32>::from_array(([1usize, h as usize, w as usize, 3], self.tensor.clone()))?;
        let m = &mut self.model;
        let outputs = m
            .session
            .run(ort::inputs![m.input_name.as_str() => input])?;
        let (shape, data) = outputs[m.output_name.as_str()].try_extract_tensor::<f32>()?;
        let dims: &[i64] = shape;
        ensure!(
            dims == [1, h as i64, w as i64, 1],
            "unexpected model output shape {dims:?}"
        );
        postprocess(data, w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_is_nhwc_unit_range() {
        let mut t = [0.0; 6];
        preprocess(&[255, 0, 51, 255, 0, 255, 0, 7], &mut t);
        assert_eq!(t, [1.0, 0.0, 0.2, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn postprocess_keeps_absolute_probabilities() {
        // No min-max stretch: a low-confidence frame stays low.
        let m = postprocess(&[0.0, 0.1, 0.2, 0.5], 2, 2).unwrap();
        assert_eq!(m.data, [0, 26, 51, 128]);
    }
}
