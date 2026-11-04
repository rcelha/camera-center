//! Runs the bundled models on a synthetic frame. Skipped (with a message) if
//! the model files are not present, e.g. when Git LFS objects weren't pulled.

use cc_core::{Frame, SegmentationModel, Segmenter};
use cc_ml::{ExecutionBackend, U2Net, model_path};

fn synthetic_portrait() -> Frame {
    // A bright "head and shoulders" blob on a dark background.
    let (w, h) = (640u32, 480u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - 320.0, y as f32 - 200.0);
            let head = dx * dx / 90.0f32.powi(2) + dy * dy / 115.0f32.powi(2) < 1.0;
            let body = y > 300 && (x as f32 - 320.0).abs() < 200.0;
            if head || body {
                data.extend_from_slice(&[225, 180, 150, 255]);
            } else {
                data.extend_from_slice(&[40, 60, 70, 255]);
            }
        }
    }
    Frame::new(w, h, data).unwrap()
}

#[test]
fn models_produce_masks() {
    for model in SegmentationModel::ALL {
        let path = match model_path(model) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping {model:?}: {e:#}");
                continue;
            }
        };
        let mut seg = U2Net::load(&path, ExecutionBackend::Cpu).unwrap();
        let mask = seg.segment(&synthetic_portrait()).unwrap();
        assert_eq!((mask.width, mask.height), seg.input_size());
        assert!(mask.data.iter().any(|&v| v > 128), "{model:?}: empty mask");
        assert!(mask.data.iter().any(|&v| v < 128), "{model:?}: full mask");
    }
}
