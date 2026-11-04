//! `cargo run -p cc-ml --release --example bench` — per-model inference latency.

use std::time::Instant;

use cc_core::{Frame, SegmentationModel, Segmenter};
use cc_ml::{ExecutionBackend, U2Net, model_path};

fn main() -> anyhow::Result<()> {
    let frame = Frame::filled(320, 320, [120, 100, 90, 255]);
    for model in SegmentationModel::ALL {
        for backend in [ExecutionBackend::Cpu, ExecutionBackend::Auto] {
            let t = Instant::now();
            let mut seg = U2Net::load(&model_path(model)?, backend)?;
            let load = t.elapsed();
            seg.segment(&frame)?; // warm-up
            let n = 10;
            let t = Instant::now();
            for _ in 0..n {
                seg.segment(&frame)?;
            }
            println!(
                "{model:?} {backend:?}: load {:?}, {:.1} ms/frame",
                load,
                t.elapsed().as_secs_f64() * 1000.0 / n as f64
            );
        }
    }
    Ok(())
}
