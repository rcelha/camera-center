//! ONNX Runtime environment and session setup shared by every model.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use tracing::info;

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

fn build_session(path: &Path, backend: ExecutionBackend) -> ort::Result<Session> {
    let env = environment().map_err(|e| ort::Error::new(e.to_string()))?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(4));
    let builder = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(threads)?;
    let mut builder = match backend {
        // ort's SessionBuilder defaults to the MaxEfficiency device policy, which
        // attaches CoreML on macOS even with no providers registered.
        ExecutionBackend::Cpu => builder.with_auto_device(AutoDevicePolicy::PreferCPU)?,
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

/// A loaded model with a single input and a single output.
pub(crate) struct ModelSession {
    pub session: Session,
    pub input_name: String,
    pub output_name: String,
    /// Static input shape as declared by the model.
    pub input_shape: Vec<i64>,
}

impl ModelSession {
    pub fn load(path: &Path, backend: ExecutionBackend) -> Result<Self> {
        let started = std::time::Instant::now();
        let session = build_session(path, backend)
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("failed to load model {}", path.display()))?;
        let input = session.inputs().first().context("model has no inputs")?;
        let input_name = input.name().to_string();
        let input_shape = input
            .dtype()
            .tensor_shape()
            .context("model input is not a tensor")?
            .to_vec();
        let output_name = session
            .outputs()
            .first()
            .context("model has no outputs")?
            .name()
            .to_string();
        info!(
            model = %path.display(),
            ?backend,
            ?input_shape,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "segmentation model loaded"
        );
        Ok(Self {
            session,
            input_name,
            output_name,
            input_shape,
        })
    }
}
