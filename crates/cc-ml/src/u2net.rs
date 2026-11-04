//! U²-Net family (u2net_human_seg, u2netp) inference.
//!
//! Pre/post-processing follows rembg: RGB scaled by the image max, ImageNet
//! mean/std normalisation, NCHW 1x3x320x320 input; the first output (d1) is
//! min-max normalised into the mask.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, ensure};
use cc_core::{Frame, Mask, Segmenter, resize};
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use tracing::info;

const INPUT: u32 = 320;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Which hardware ONNX Runtime should try first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionBackend {
    /// Platform accelerator (CoreML on macOS, DirectML on Windows) with CPU fallback.
    Auto,
    Cpu,
}

fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .with_name("camera-center")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| anyhow!("failed to initialise ONNX Runtime: {e}"))
}

pub struct U2Net {
    session: Session,
    input_name: String,
    output_name: String,
    tensor: Vec<f32>,
}

fn build_session(path: &Path, backend: ExecutionBackend) -> ort::Result<Session> {
    let env = environment().map_err(|e| ort::Error::new(e.to_string()))?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(4));
    let builder = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(threads)?;
    let mut builder = match backend {
        ExecutionBackend::Cpu => builder,
        ExecutionBackend::Auto => builder.with_execution_providers(platform_providers())?,
    };
    builder.commit_from_file(path)
}

#[cfg(target_os = "macos")]
fn platform_providers() -> Vec<ort::ep::ExecutionProviderDispatch> {
    let cache = std::env::temp_dir().join("camera-center-coreml-cache");
    vec![
        ort::ep::CoreML::default()
            .with_static_input_shapes(true)
            .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
            // Measured on M3 Max: ANE is ~1.5x faster than CPU for u2netp and on
            // par for u2net_human_seg, while GPU / ALL were slower than CPU.
            .with_compute_units(ort::ep::coreml::ComputeUnits::CPUAndNeuralEngine)
            .with_model_cache_dir(cache.display())
            .build(),
    ]
}

#[cfg(windows)]
fn platform_providers() -> Vec<ort::ep::ExecutionProviderDispatch> {
    vec![ort::ep::DirectML::default().build()]
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_providers() -> Vec<ort::ep::ExecutionProviderDispatch> {
    Vec::new()
}

impl U2Net {
    pub fn load(path: &Path, backend: ExecutionBackend) -> Result<Self> {
        let started = std::time::Instant::now();
        let session = build_session(path, backend)
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("failed to load model {}", path.display()))?;
        let input_name = session
            .inputs()
            .first()
            .context("model has no inputs")?
            .name()
            .to_string();
        let output_name = session
            .outputs()
            .first()
            .context("model has no outputs")?
            .name()
            .to_string();
        info!(
            model = %path.display(),
            ?backend,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "segmentation model loaded"
        );
        Ok(Self {
            session,
            input_name,
            output_name,
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
        let rgba = if (frame.width, frame.height) == (INPUT, INPUT) {
            std::borrow::Cow::Borrowed(&frame.data)
        } else {
            std::borrow::Cow::Owned(resize::rgba(
                &frame.data,
                frame.width,
                frame.height,
                INPUT,
                INPUT,
            )?)
        };
        self.preprocess(&rgba);
        let input = Tensor::<f32>::from_array((
            [1usize, 3, INPUT as usize, INPUT as usize],
            self.tensor.clone(),
        ))?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => input])?;
        let (shape, data) = outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
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
