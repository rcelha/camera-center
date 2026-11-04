use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::warn;

use super::{Background, FrameContext};
use crate::pipeline::{Step, StepId};
use crate::{AppConfig, Frame, blur, resize};

/// Largest background image we decode (pixels per side).
const MAX_IMAGE_SIDE: u32 = 8192;

/// Maps the 0..=100 strength setting to a Gaussian sigma for a frame size.
pub fn blur_sigma(strength: u8, width: u32, height: u32) -> f32 {
    let max_sigma = width.max(height) as f32 * 0.025;
    strength.min(100) as f32 / 100.0 * max_sigma
}

/// Replaces the background layer with a user supplied image (cover fit).
#[derive(Default)]
pub struct BackgroundImage {
    /// Decoded source image, keyed by path.
    source: Option<(PathBuf, Arc<image::RgbaImage>)>,
    /// Path that failed to load, so we only warn once.
    failed: Option<PathBuf>,
    /// Source scaled to the current frame size.
    scaled: Option<(u32, u32, Arc<Vec<u8>>)>,
}

impl BackgroundImage {
    fn source_for(&mut self, path: &Path) -> Option<Arc<image::RgbaImage>> {
        if let Some((p, img)) = &self.source
            && p == path
        {
            return Some(img.clone());
        }
        if self.failed.as_deref() == Some(path) {
            return None;
        }
        match load_image(path) {
            Ok(img) => {
                let img = Arc::new(img);
                self.source = Some((path.to_path_buf(), img.clone()));
                self.scaled = None;
                self.failed = None;
                Some(img)
            }
            Err(e) => {
                warn!("background image unavailable: {e:#}");
                self.failed = Some(path.to_path_buf());
                None
            }
        }
    }
}

/// Decodes an image with dimension limits so a hostile file can't exhaust memory.
pub fn load_image(path: &Path) -> Result<image::RgbaImage> {
    let mut reader = image::ImageReader::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?
        .with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    reader.limits(limits);
    let img = reader
        .decode()
        .with_context(|| format!("cannot decode {}", path.display()))?;
    Ok(img.into_rgba8())
}

impl Step for BackgroundImage {
    fn id(&self) -> StepId {
        StepId::BackgroundImage
    }

    fn enabled(&self, config: &AppConfig) -> bool {
        config.effects.background_image && config.background_image.is_some()
    }

    fn needs_mask(&self) -> bool {
        true
    }

    fn process(&mut self, frame: &mut Frame, ctx: &mut FrameContext<'_>) -> Result<()> {
        let Some(path) = ctx.config.background_image.as_deref() else {
            return Ok(());
        };
        let Some(src) = self.source_for(path) else {
            return Ok(());
        };
        let (w, h) = (frame.width, frame.height);
        let scaled = match &self.scaled {
            Some((sw, sh, buf)) if *sw == w && *sh == h => buf.clone(),
            _ => {
                let buf = Arc::new(resize::rgba_cover(
                    src.as_raw(),
                    src.width(),
                    src.height(),
                    w,
                    h,
                )?);
                self.scaled = Some((w, h, buf.clone()));
                buf
            }
        };
        ctx.background = Background::Shared(scaled);
        Ok(())
    }
}

/// Blurs the background layer. If a background image is active, the image is
/// blurred once and cached; otherwise the camera frame is blurred every frame.
#[derive(Default)]
pub struct BlurBackground {
    /// (source buffer identity, sigma bits, blurred result)
    cached: Option<(usize, u32, Arc<Vec<u8>>)>,
}

impl Step for BlurBackground {
    fn id(&self) -> StepId {
        StepId::BlurBackground
    }

    fn enabled(&self, config: &AppConfig) -> bool {
        config.effects.blur_background && config.blur_strength > 0
    }

    fn needs_mask(&self) -> bool {
        true
    }

    fn process(&mut self, frame: &mut Frame, ctx: &mut FrameContext<'_>) -> Result<()> {
        let (w, h) = (frame.width, frame.height);
        let sigma = blur_sigma(ctx.config.blur_strength, w, h);
        ctx.background = match std::mem::take(&mut ctx.background) {
            Background::Camera => Background::Owned(blur::rgba(&frame.data, w, h, sigma)?),
            Background::Owned(buf) => Background::Owned(blur::rgba(&buf, w, h, sigma)?),
            Background::Shared(buf) => {
                let key = (Arc::as_ptr(&buf) as usize, sigma.to_bits());
                match &self.cached {
                    Some((ptr, s, blurred)) if (*ptr, *s) == key => {
                        Background::Shared(blurred.clone())
                    }
                    _ => {
                        let blurred = Arc::new(blur::rgba(&buf, w, h, sigma)?);
                        self.cached = Some((key.0, key.1, blurred.clone()));
                        Background::Shared(blurred)
                    }
                }
            }
        };
        Ok(())
    }
}
