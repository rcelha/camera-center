use anyhow::{Result, ensure};
use rayon::prelude::*;

use super::FrameContext;
use crate::pipeline::{Step, StepId};
use crate::{AppConfig, Frame};

/// Blends `fg` over `bg` in place: `fg = fg * m + bg * (1 - m)`.
pub fn composite(fg: &mut [u8], bg: &[u8], mask: &[u8]) -> Result<()> {
    ensure!(
        fg.len() == bg.len() && fg.len() == mask.len() * 4,
        "composite buffers differ in size"
    );
    const CHUNK: usize = 4096;
    fg.par_chunks_mut(CHUNK * 4)
        .zip(bg.par_chunks(CHUNK * 4))
        .zip(mask.par_chunks(CHUNK))
        .for_each(|((fg, bg), mask)| {
            for ((f, b), &m) in fg.chunks_exact_mut(4).zip(bg.chunks_exact(4)).zip(mask) {
                let a = m as u32;
                let ia = 255 - a;
                for c in 0..3 {
                    f[c] = div255(f[c] as u32 * a + b[c] as u32 * ia);
                }
                f[3] = 255;
            }
        });
    Ok(())
}

#[inline]
pub(crate) fn div255(v: u32) -> u8 {
    ((v + 128 + ((v + 128) >> 8)) >> 8) as u8
}

/// Blends the person over the background layer built by previous steps.
#[derive(Default)]
pub struct Composite;

impl Step for Composite {
    fn id(&self) -> StepId {
        StepId::Composite
    }

    fn enabled(&self, _config: &AppConfig) -> bool {
        true
    }

    fn process(&mut self, frame: &mut Frame, ctx: &mut FrameContext<'_>) -> Result<()> {
        let (Some(bg), Some(mask)) = (ctx.background.as_slice(), ctx.mask) else {
            return Ok(());
        };
        composite(&mut frame.data, bg, &mask.data)?;
        ctx.background_replaced = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div255_matches_exact_rounding() {
        for v in (0..=255 * 255).step_by(7) {
            let exact = (v as f32 / 255.0).round() as i32;
            assert!((div255(v) as i32 - exact).abs() <= 1, "{v}");
        }
        assert_eq!(div255(255 * 255), 255);
        assert_eq!(div255(0), 0);
    }

    #[test]
    fn mask_selects_layers() {
        let mut fg = vec![200, 200, 200, 255, 200, 200, 200, 255];
        let bg = vec![0, 0, 0, 255, 0, 0, 0, 255];
        composite(&mut fg, &bg, &[255, 0]).unwrap();
        assert_eq!(fg, [200, 200, 200, 255, 0, 0, 0, 255]);
    }
}
