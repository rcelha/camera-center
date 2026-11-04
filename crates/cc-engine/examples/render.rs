//! `cargo run -p cc-engine --release --example render -- <input.png|test-pattern> <out-dir> [bg.png]`
//!
//! Runs segmentation + every effect combination on one image and writes PNGs,
//! handy for eyeballing effect changes without a camera.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use cc_core::{AppConfig, EffectToggles, Frame, Pipeline, SegmentationModel, Segmenter};
use cc_engine::{FrameSource, TestPattern};

fn save(frame: &Frame, path: &std::path::Path) -> Result<()> {
    image::RgbaImage::from_raw(frame.width, frame.height, frame.data.clone())
        .context("bad frame")?
        .save(path)?;
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().unwrap_or_else(|| "test-pattern".into());
    let out = PathBuf::from(args.next().unwrap_or_else(|| "render-out".into()));
    let bg = args.next().map(PathBuf::from);
    std::fs::create_dir_all(&out)?;

    let frame = if input == "test-pattern" {
        TestPattern::new(Some(720)).next_frame()?
    } else {
        let img = image::open(&input)?.into_rgba8();
        Frame::new(img.width(), img.height(), img.into_raw())?
    };
    save(&frame, &out.join("0-original.png"))?;

    for model in SegmentationModel::ALL {
        let mut seg =
            cc_ml::U2Net::load(&cc_ml::model_path(model)?, cc_ml::ExecutionBackend::Auto)?;
        let mask = Arc::new(seg.segment(&frame)?);
        let m = mask.resized(frame.width, frame.height)?;
        let mask_img = Frame::new(
            m.width,
            m.height,
            m.data.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        )?;
        save(&mask_img, &out.join(format!("mask-{model:?}.png")))?;

        let combos = [
            (
                "blur",
                EffectToggles {
                    blur_background: true,
                    ..Default::default()
                },
            ),
            (
                "image",
                EffectToggles {
                    background_image: true,
                    ..Default::default()
                },
            ),
            (
                "image+blur",
                EffectToggles {
                    background_image: true,
                    blur_background: true,
                    ..Default::default()
                },
            ),
            (
                "portrait",
                EffectToggles {
                    portrait: true,
                    ..Default::default()
                },
            ),
            (
                "blur+portrait",
                EffectToggles {
                    blur_background: true,
                    portrait: true,
                    ..Default::default()
                },
            ),
        ];
        for (name, effects) in combos {
            if effects.background_image && bg.is_none() {
                continue;
            }
            let cfg = AppConfig {
                effects,
                blur_strength: 60,
                background_image: bg.clone(),
                model,
                ..Default::default()
            };
            let mut f = frame.clone();
            Pipeline::new().process(&mut f, Some(&mask), &cfg)?;
            save(&f, &out.join(format!("{model:?}-{name}.png")))?;
        }
    }
    println!("wrote {}", out.display());
    Ok(())
}
