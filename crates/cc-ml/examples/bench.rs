//! `cargo run -p cc-ml --release --example bench` — per-model inference latency.

use std::time::Instant;

use cc_core::{Frame, SegmentationModel};
use cc_ml::ExecutionBackend;

fn main() -> anyhow::Result<()> {
    for model in SegmentationModel::ALL {
        let (w, h) = model.input_size();
        let frame = Frame::filled(w, h, [120, 100, 90, 255]);
        for backend in [ExecutionBackend::Cpu, ExecutionBackend::Auto] {
            let t = Instant::now();
            let mut seg = cc_ml::load(model, backend)?;
            let load = t.elapsed();
            seg.segment(&frame)?; // warm-up
            let n = 30;
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
