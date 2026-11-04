//! The processing pipeline: an ordered list of [`Step`]s toggled by config.

use std::sync::Arc;

use anyhow::Result;

use crate::effects::{
    Background, BackgroundImage, BlurBackground, Composite, FrameContext, Portrait,
};
use crate::{AppConfig, Frame, Mask};

/// Identifies a pipeline step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepId {
    BackgroundImage,
    BlurBackground,
    Composite,
    Portrait,
}

/// A single stage of frame processing.
pub trait Step: Send {
    fn id(&self) -> StepId;

    /// Whether the step runs for the given configuration (its toggle).
    fn enabled(&self, config: &AppConfig) -> bool;

    /// Whether the step requires a segmentation mask.
    fn needs_mask(&self) -> bool {
        false
    }

    fn process(&mut self, frame: &mut Frame, ctx: &mut FrameContext<'_>) -> Result<()>;
}

/// Ordered collection of steps.
pub struct Pipeline {
    steps: Vec<Box<dyn Step>>,
    /// Mask resized to the last frame size, reused while the source mask is
    /// unchanged. Holding the source `Arc` keeps the identity check sound.
    scaled_mask: Option<(Arc<Mask>, u32, u32, Arc<Mask>)>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    /// The standard pipeline: background image → blur → composite → portrait.
    pub fn new() -> Self {
        Self::with_steps(vec![
            Box::new(BackgroundImage::default()),
            Box::new(BlurBackground::default()),
            Box::new(Composite),
            Box::new(Portrait::default()),
        ])
    }

    pub fn with_steps(steps: Vec<Box<dyn Step>>) -> Self {
        Self {
            steps,
            scaled_mask: None,
        }
    }

    pub fn step_ids(&self) -> Vec<StepId> {
        self.steps.iter().map(|s| s.id()).collect()
    }

    /// True if any enabled step needs segmentation.
    pub fn needs_mask(&self, config: &AppConfig) -> bool {
        self.steps
            .iter()
            .any(|s| s.needs_mask() && s.enabled(config))
    }

    /// Runs every enabled step on `frame` in place.
    ///
    /// `mask` may be at any resolution (it is resized to the frame and cached
    /// while the same `Arc` is passed again).
    pub fn process(
        &mut self,
        frame: &mut Frame,
        mask: Option<&Arc<Mask>>,
        config: &AppConfig,
    ) -> Result<()> {
        if !self
            .steps
            .iter()
            .any(|s| s.id() != StepId::Composite && s.enabled(config))
        {
            return Ok(());
        }
        let scaled = match mask {
            Some(m) => Some(self.scaled_mask(m, frame.width, frame.height)?),
            None => None,
        };
        let mut ctx = FrameContext {
            mask: scaled.as_deref(),
            config,
            background: Background::Camera,
            background_replaced: false,
        };
        for step in &mut self.steps {
            if step.enabled(config) {
                step.process(frame, &mut ctx)?;
            }
        }
        Ok(())
    }

    fn scaled_mask(&mut self, mask: &Arc<Mask>, w: u32, h: u32) -> Result<Arc<Mask>> {
        if let Some((src, sw, sh, scaled)) = &self.scaled_mask
            && Arc::ptr_eq(src, mask)
            && (*sw, *sh) == (w, h)
        {
            return Ok(scaled.clone());
        }
        let scaled = Arc::new(mask.resized(w, h)?);
        self.scaled_mask = Some((mask.clone(), w, h, scaled.clone()));
        Ok(scaled)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::EffectToggles;

    fn split_mask(w: u32, h: u32) -> Arc<Mask> {
        // Left half is the person.
        let data = (0..h)
            .flat_map(|_| (0..w).map(move |x| if x < w / 2 { 255 } else { 0 }))
            .collect();
        Arc::new(Mask::new(w, h, data).unwrap())
    }

    fn noisy_frame(w: u32, h: u32) -> Frame {
        let data = (0..w * h)
            .flat_map(|i| {
                let v = if (i % w + i / w).is_multiple_of(2) {
                    220
                } else {
                    30
                };
                [v, v, v, 255]
            })
            .collect();
        Frame::new(w, h, data).unwrap()
    }

    fn px(f: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * f.width + x) * 4) as usize;
        f.data[i..i + 4].try_into().unwrap()
    }

    fn write_bg(name: &str, rgba: [u8; 4]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("cc-{name}-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(32, 32, image::Rgba(rgba))
            .save(&path)
            .unwrap();
        path
    }

    #[test]
    fn order_is_fixed() {
        assert_eq!(
            Pipeline::new().step_ids(),
            [
                StepId::BackgroundImage,
                StepId::BlurBackground,
                StepId::Composite,
                StepId::Portrait
            ]
        );
    }

    #[test]
    fn no_effects_is_passthrough() {
        let mut f = noisy_frame(16, 16);
        let orig = f.clone();
        Pipeline::new()
            .process(&mut f, None, &AppConfig::default())
            .unwrap();
        assert_eq!(f, orig);
        assert!(!Pipeline::new().needs_mask(&AppConfig::default()));
    }

    #[test]
    fn blur_only_touches_background() {
        let cfg = AppConfig {
            blur_strength: 100,
            effects: EffectToggles {
                blur_background: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut p = Pipeline::new();
        assert!(p.needs_mask(&cfg));
        let mut f = noisy_frame(64, 32);
        let orig = f.clone();
        p.process(&mut f, Some(&split_mask(64, 32)), &cfg).unwrap();
        // Person side untouched, background side smoothed towards the mean.
        assert_eq!(px(&f, 3, 3), px(&orig, 3, 3));
        let bg = px(&f, 50, 16)[0];
        assert!((90..=160).contains(&bg), "blurred value {bg}");
    }

    #[test]
    fn background_image_replaces_background() {
        let path = write_bg("bgimg", [0, 200, 0, 255]);
        let cfg = AppConfig {
            background_image: Some(path.clone()),
            effects: EffectToggles {
                background_image: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut f = noisy_frame(64, 32);
        let orig = f.clone();
        Pipeline::new()
            .process(&mut f, Some(&split_mask(64, 32)), &cfg)
            .unwrap();
        assert_eq!(px(&f, 3, 3), px(&orig, 3, 3));
        assert_eq!(px(&f, 60, 16), [0, 200, 0, 255]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn blur_and_image_blurs_the_image_once() {
        // Half red / half blue image so blurring is observable at the seam.
        let path = std::env::temp_dir().join(format!("cc-bgseam-{}.png", std::process::id()));
        let img = image::RgbaImage::from_fn(64, 32, |x, _| {
            if x < 32 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        img.save(&path).unwrap();
        let cfg = AppConfig {
            background_image: Some(path.clone()),
            blur_strength: 100,
            effects: EffectToggles {
                background_image: true,
                blur_background: true,
                portrait: false,
            },
            ..Default::default()
        };
        let mask = Arc::new(Mask::filled(64, 32, 0)); // everything is background
        let mut p = Pipeline::new();
        let mut a = noisy_frame(64, 32);
        p.process(&mut a, Some(&mask), &cfg).unwrap();
        // Seam is blended → blurred image, not the camera.
        let seam = px(&a, 32, 16);
        assert!(seam[0] > 30 && seam[2] > 30, "seam {seam:?}");
        // Camera content never shows through.
        let mut b = noisy_frame(64, 32);
        p.process(&mut b, Some(&mask), &cfg).unwrap();
        assert_eq!(a, b, "cached blurred image must be stable across frames");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn missing_mask_is_passthrough_for_background_effects() {
        let cfg = AppConfig {
            effects: EffectToggles {
                blur_background: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut f = noisy_frame(16, 16);
        let orig = f.clone();
        Pipeline::new().process(&mut f, None, &cfg).unwrap();
        assert_eq!(f, orig);
    }

    #[test]
    fn portrait_runs_without_mask() {
        let cfg = AppConfig {
            effects: EffectToggles {
                portrait: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut f = Frame::filled(64, 64, [60, 60, 60, 255]);
        Pipeline::new().process(&mut f, None, &cfg).unwrap();
        // Dark subject gets brightened at the centre, corners vignetted.
        assert!(px(&f, 32, 32)[0] > 60);
        assert!(px(&f, 0, 0)[0] < px(&f, 32, 32)[0]);
    }

    #[test]
    fn mask_is_rescaled_to_frame() {
        let cfg = AppConfig {
            effects: EffectToggles {
                blur_background: true,
                ..Default::default()
            },
            blur_strength: 80,
            ..Default::default()
        };
        let mut f = noisy_frame(128, 64);
        Pipeline::new()
            .process(&mut f, Some(&split_mask(32, 16)), &cfg)
            .unwrap();
    }
}
