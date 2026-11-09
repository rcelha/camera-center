//! Runs the bundled models on a real portrait. Skipped (with a message) if
//! the model files are not present, e.g. when Git LFS objects weren't pulled.
//!
//! `data/portrait.jpg` is the 2012 official White House portrait (Pete Souza),
//! a US government work in the public domain, downscaled from the copy in
//! MediaPipe's test assets (storage.googleapis.com/mediapipe-assets/portrait.jpg).

use cc_core::{Frame, Mask, SegmentationModel};
use cc_ml::{ExecutionBackend, model_path};

fn portrait() -> Frame {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/portrait.jpg");
    let img = image::open(path).unwrap().into_rgba8();
    Frame::new(img.width(), img.height(), img.into_raw()).unwrap()
}

/// Mean mask value over a rectangle given in fractions of the mask size.
fn mean(mask: &Mask, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let (w, h) = (mask.width as f32, mask.height as f32);
    let (mut sum, mut n) = (0u64, 0u64);
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            sum += u64::from(mask.data[(y * mask.width + x) as usize]);
            n += 1;
        }
    }
    sum as f32 / n as f32
}

#[test]
fn models_segment_a_portrait() {
    let frame = portrait();
    for model in SegmentationModel::ALL {
        if let Err(e) = model_path(model) {
            eprintln!("skipping {model:?}: {e:#}");
            continue;
        }
        // Auto also exercises the platform accelerator (CoreML / DirectML).
        for backend in [ExecutionBackend::Cpu, ExecutionBackend::Auto] {
            let mut seg = cc_ml::load(model, backend).unwrap();
            assert_eq!(seg.input_size(), model.input_size(), "{model:?}");
            let mask = seg.segment(&frame).unwrap();
            assert_eq!((mask.width, mask.height), seg.input_size(), "{model:?}");
            check_portrait_mask(&format!("{model:?} {backend:?}"), &mask);
        }
    }
}

fn check_portrait_mask(name: &str, mask: &Mask) {
    let face = mean(mask, 0.4, 0.15, 0.6, 0.35);
    let chest = mean(mask, 0.35, 0.6, 0.65, 0.8);
    let corners = (mean(mask, 0.0, 0.0, 0.12, 0.25) + mean(mask, 0.88, 0.0, 1.0, 0.25)) / 2.0;
    eprintln!("{name}: face {face:.0}, chest {chest:.0}, corners {corners:.0}");
    assert!(face > 200.0, "{name}: face not foreground ({face})");
    assert!(chest > 200.0, "{name}: chest not foreground ({chest})");
    assert!(corners < 40.0, "{name}: corners not background ({corners})");
}
