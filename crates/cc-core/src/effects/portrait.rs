//! "Portrait" look for video calls.
//!
//! * a shallow depth-of-field blur whose strength ramps up with distance
//!   from the subject's silhouette (only when no other background effect ran),
//! * subject exposure normalisation, mild contrast and warmth (LUT based),
//! * a soft vignette.

use anyhow::Result;
use rayon::prelude::*;

use super::{FrameContext, composite::div255};
use crate::pipeline::{Step, StepId};
use crate::{AppConfig, Frame, blur};

/// Depth blur as a fraction of the larger frame side.
const DEPTH_BLUR: f32 = 0.009;
/// Feathering of the mask for the graded depth-of-field falloff.
const DEPTH_FEATHER: f32 = 0.02;
/// Target mean luma of the subject (0..1).
const TARGET_LUMA: f32 = 0.47;
/// Vignette darkening at the corners.
const VIGNETTE: f32 = 0.22;

#[derive(Default)]
pub struct Portrait {
    vignette: Option<(u32, u32, Vec<u16>)>,
    /// Exposure gain smoothed over time so the image doesn't pump.
    gain: Option<f32>,
}

impl Portrait {
    fn vignette(&mut self, w: u32, h: u32) -> &[u16] {
        let stale = !matches!(&self.vignette, Some((vw, vh, _)) if (*vw, *vh) == (w, h));
        if stale {
            self.vignette = Some((w, h, vignette_map(w, h)));
        }
        &self.vignette.as_ref().map(|v| &v.2).expect("set above")[..]
    }
}

/// Per-pixel gain in 1/256 units: 256 = unchanged.
fn vignette_map(w: u32, h: u32) -> Vec<u16> {
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            let dx = (x as f32 + 0.5 - cx) / cx;
            let dy = (y as f32 + 0.5 - cy) / cy;
            let r = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
            let t = ((r - 0.45) / 0.55).clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t);
            out.push(((1.0 - VIGNETTE * s) * 256.0) as u16);
        }
    }
    out
}

/// Mean luma of the subject (mask weighted), sampled sparsely.
fn subject_luma(frame: &Frame, mask: Option<&[u8]>) -> f32 {
    let (mut sum, mut weight) = (0f64, 0f64);
    for i in (0..frame.pixel_count()).step_by(7) {
        let p = &frame.data[i * 4..i * 4 + 3];
        let w = mask.map_or(1.0, |m| m[i] as f64 / 255.0);
        let l = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
        sum += l * w;
        weight += w;
    }
    if weight < 1.0 {
        return TARGET_LUMA;
    }
    (sum / weight / 255.0) as f32
}

/// Builds the tone LUT for each channel.
fn tone_luts(gain: f32) -> [[u8; 256]; 3] {
    let warmth = [1.035f32, 1.01, 0.965];
    let mut luts = [[0u8; 256]; 3];
    for (c, lut) in luts.iter_mut().enumerate() {
        for (v, out) in lut.iter_mut().enumerate() {
            let x = (v as f32 / 255.0 * gain * warmth[c]).clamp(0.0, 1.0);
            // Gentle S-curve for contrast.
            let s = x * x * (3.0 - 2.0 * x);
            let y = x + (s - x) * 0.3;
            *out = (y * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    luts
}

impl Step for Portrait {
    fn id(&self) -> StepId {
        StepId::Portrait
    }

    fn enabled(&self, config: &AppConfig) -> bool {
        config.effects.portrait
    }

    fn needs_mask(&self) -> bool {
        true
    }

    fn process(&mut self, frame: &mut Frame, ctx: &mut FrameContext<'_>) -> Result<()> {
        let (w, h) = (frame.width, frame.height);
        let side = w.max(h) as f32;

        // 1. Graded depth-of-field, only if the background is still the camera's.
        if !ctx.background_replaced
            && let Some(mask) = ctx.mask
        {
            let blurred = blur::rgba(&frame.data, w, h, side * DEPTH_BLUR)?;
            // Erode-ish feather: blur then bias towards background so the
            // falloff starts just outside the silhouette.
            let mut soft = blur::gray(&mask.data, w, h, side * DEPTH_FEATHER)?;
            soft.par_iter_mut()
                .zip(mask.data.par_iter())
                .for_each(|(s, &m)| *s = (*s).max(m));
            super::composite(&mut frame.data, &blurred, &soft)?;
        }

        // 2. Subject exposure + tone, weighted by the mask.
        let mask = ctx.mask.map(|m| &m.data[..]);
        let measured = subject_luma(frame, mask).max(0.02);
        let target_gain = (TARGET_LUMA / measured).clamp(0.85, 1.35);
        let gain = match self.gain {
            Some(g) => g + (target_gain - g) * 0.08,
            None => target_gain,
        };
        self.gain = Some(gain);
        let luts = tone_luts(gain);

        let vignette = self.vignette(w, h).to_vec();
        let width = w as usize;
        frame
            .data
            .par_chunks_mut(width * 4)
            .enumerate()
            .for_each(|(y, row)| {
                for x in 0..width {
                    let i = y * width + x;
                    // Background keeps 40% of the grade so the frame stays coherent.
                    let wgt = mask.map_or(255u32, |m| 102 + (m[i] as u32 * 153) / 255);
                    let vg = vignette[i] as u32;
                    let p = &mut row[x * 4..x * 4 + 3];
                    for c in 0..3 {
                        let orig = p[c] as u32;
                        let graded = luts[c][p[c] as usize] as u32;
                        let mixed = div255(graded * wgt + orig * (255 - wgt)) as u32;
                        p[c] = ((mixed * vg) >> 8).min(255) as u8;
                    }
                }
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vignette_darkens_corners_only() {
        let v = vignette_map(100, 100);
        assert_eq!(v[50 * 100 + 50], 256);
        assert!(v[0] < 220);
    }

    #[test]
    fn luts_are_monotonic() {
        for lut in tone_luts(1.2) {
            assert!(lut.windows(2).all(|w| w[0] <= w[1]));
        }
    }
}
