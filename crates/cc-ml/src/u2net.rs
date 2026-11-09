//! U²-Net family (u2net_human_seg, u2netp) inference.
//!
//! Pre/post-processing follows rembg: RGB scaled by the image max, ImageNet
//! mean/std normalisation, NCHW 1x3x320x320 input; the first output (d1) is
//! min-max normalised into the mask.

use std::path::Path;

use anyhow::{Result, ensure};
use cc_core::{Frame, Mask, Segmenter};
use ort::value::Tensor;

use crate::session::{ExecutionBackend, ModelSession};

const INPUT: u32 = 320;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

pub struct U2Net {
    model: ModelSession,
    tensor: Vec<f32>,
}

impl U2Net {
    pub fn load(path: &Path, backend: ExecutionBackend) -> Result<Self> {
        let model = ModelSession::load(path, backend)?;
        ensure!(
            model.input_shape == [1, 3, INPUT as i64, INPUT as i64],
            "{} is not a U²-Net model: input shape {:?}",
            path.display(),
            model.input_shape
        );
        Ok(Self {
            model,
            tensor: vec![0.0; (3 * INPUT * INPUT) as usize],
        })
    }

    /// Fills `self.tensor` (NCHW) from a 320x320 RGBA buffer.
    fn preprocess(&mut self, rgba: &[u8]) {
        let plane = (INPUT * INPUT) as usize;
        let max = rgba
            .chunks_exact(4)
            .flat_map(|p| p[..3].iter().copied())
            .max()
            .unwrap_or(255)
            .max(1) as f32;
        for (i, p) in rgba.chunks_exact(4).enumerate() {
            for c in 0..3 {
                self.tensor[c * plane + i] = (p[c] as f32 / max - MEAN[c]) / STD[c];
            }
        }
    }
}

/// Min-max normalises raw model output into a mask.
fn postprocess(raw: &[f32], width: u32, height: u32) -> Result<Mask> {
    let (min, max) = raw
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let range = (max - min).max(1e-6);
    let data = raw
        .iter()
        .map(|&v| (((v - min) / range) * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect();
    Mask::new(width, height, data)
}

impl Segmenter for U2Net {
    fn input_size(&self) -> (u32, u32) {
        (INPUT, INPUT)
    }

    fn segment(&mut self, frame: &Frame) -> Result<Mask> {
        let rgba = crate::scaled_rgba(frame, INPUT, INPUT)?;
        self.preprocess(&rgba);
        let input = Tensor::<f32>::from_array((
            [1usize, 3, INPUT as usize, INPUT as usize],
            self.tensor.clone(),
        ))?;
        let m = &mut self.model;
        let outputs = m
            .session
            .run(ort::inputs![m.input_name.as_str() => input])?;
        let (shape, data) = outputs[m.output_name.as_str()].try_extract_tensor::<f32>()?;
        let dims: &[i64] = shape;
        ensure!(
            dims.len() == 4 && dims[2] == INPUT as i64 && dims[3] == INPUT as i64,
            "unexpected model output shape {dims:?}"
        );
        postprocess(data, INPUT, INPUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postprocess_normalises() {
        let m = postprocess(&[0.2, 0.4, 0.6, 1.0], 2, 2).unwrap();
        assert_eq!(m.data, [0, 64, 128, 255]);
    }
}
