//! Fast approximate Gaussian blur.
//!
//! A Gaussian is approximated by three successive box blurs (each O(n)
//! regardless of radius). Large radii are applied on a downscaled copy and
//! scaled back up, which is visually indistinguishable for background blur and
//! an order of magnitude cheaper.

use anyhow::Result;
use rayon::prelude::*;

use crate::resize;

/// Box radii whose successive application approximates a Gaussian of `sigma`.
pub fn boxes_for_gauss(sigma: f32, passes: usize) -> Vec<usize> {
    if sigma <= 0.0 {
        return vec![0; passes];
    }
    let n = passes as f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let wlf = wl as f32;
    let m_ideal =
        (12.0 * sigma * sigma - n * wlf * wlf - 4.0 * n * wlf - 3.0 * n) / (-4.0 * wlf - 4.0);
    let m = m_ideal.round() as i32;
    (0..passes as i32)
        .map(|i| {
            let size = if i < m { wl } else { wu };
            ((size - 1) / 2) as usize
        })
        .collect()
}

/// Horizontal box blur of one row of `C`-channel pixels with clamped edges.
fn box_row<const C: usize>(src: &[u8], dst: &mut [u8], radius: usize) {
    let w = src.len() / C;
    if radius == 0 || w == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let div = (2 * radius + 1) as u32;
    let half = div / 2;
    let px = |i: isize| -> usize { i.clamp(0, w as isize - 1) as usize * C };
    let mut acc = [0u32; C];
    for i in -(radius as isize)..=(radius as isize) {
        let p = px(i);
        for c in 0..C {
            acc[c] += src[p + c] as u32;
        }
    }
    for x in 0..w {
        let o = x * C;
        for c in 0..C {
            dst[o + c] = ((acc[c] + half) / div) as u8;
        }
        let add = px(x as isize + radius as isize + 1);
        let sub = px(x as isize - radius as isize);
        for c in 0..C {
            acc[c] = acc[c] + src[add + c] as u32 - src[sub + c] as u32;
        }
    }
}

/// Applies the horizontal box passes to every row in parallel.
fn blur_rows<const C: usize>(data: &mut [u8], scratch: &mut [u8], width: usize, radii: &[usize]) {
    let stride = width * C;
    data.par_chunks_mut(stride)
        .zip(scratch.par_chunks_mut(stride))
        .for_each(|(row, tmp)| {
            for &r in radii {
                box_row::<C>(row, tmp, r);
                row.copy_from_slice(tmp);
            }
        });
}

/// Transposes a `width`x`height` image of `C`-channel pixels into `dst`.
fn transpose<const C: usize>(src: &[u8], dst: &mut [u8], width: usize, height: usize) {
    // dst is height-wide, width-tall.
    dst.par_chunks_mut(height * C)
        .enumerate()
        .for_each(|(x, out_row)| {
            for y in 0..height {
                let s = (y * width + x) * C;
                out_row[y * C..y * C + C].copy_from_slice(&src[s..s + C]);
            }
        });
}

/// Full-resolution Gaussian-approximating blur, in place.
pub fn gaussian_in_place<const C: usize>(data: &mut [u8], width: usize, height: usize, sigma: f32) {
    if sigma <= 0.0 || width == 0 || height == 0 {
        return;
    }
    let radii = boxes_for_gauss(sigma, 3);
    let mut scratch = vec![0u8; data.len()];
    blur_rows::<C>(data, &mut scratch, width, &radii);
    transpose::<C>(data, &mut scratch, width, height);
    let mut scratch2 = vec![0u8; data.len()];
    blur_rows::<C>(&mut scratch, &mut scratch2, height, &radii);
    transpose::<C>(&scratch, data, height, width);
}

/// Chooses an integer downscale factor so the blur runs on a small image.
fn downscale_factor(sigma: f32, width: u32, height: u32) -> u32 {
    let by_sigma = (sigma / 3.0).floor().max(1.0) as u32;
    let by_size = (width.min(height) / 32).max(1);
    by_sigma.min(by_size).min(8)
}

/// Gaussian blur of an RGBA image with arbitrary `sigma` (in full-resolution
/// pixels). Returns a new buffer.
pub fn rgba(src: &[u8], width: u32, height: u32, sigma: f32) -> Result<Vec<u8>> {
    if sigma <= 0.0 {
        return Ok(src.to_vec());
    }
    let f = downscale_factor(sigma, width, height);
    if f == 1 {
        let mut out = src.to_vec();
        gaussian_in_place::<4>(&mut out, width as usize, height as usize, sigma);
        return Ok(out);
    }
    let (sw, sh) = ((width / f).max(1), (height / f).max(1));
    let mut small = resize::rgba_box(src, width, height, sw, sh)?;
    gaussian_in_place::<4>(&mut small, sw as usize, sh as usize, sigma / f as f32);
    resize::rgba(&small, sw, sh, width, height)
}

/// Gaussian blur of a single-channel image (used to feather masks).
pub fn gray(src: &[u8], width: u32, height: u32, sigma: f32) -> Result<Vec<u8>> {
    if sigma <= 0.0 {
        return Ok(src.to_vec());
    }
    let f = downscale_factor(sigma, width, height);
    if f == 1 {
        let mut out = src.to_vec();
        gaussian_in_place::<1>(&mut out, width as usize, height as usize, sigma);
        return Ok(out);
    }
    let (sw, sh) = ((width / f).max(1), (height / f).max(1));
    let mut small = resize::gray(src, width, height, sw, sh)?;
    gaussian_in_place::<1>(&mut small, sw as usize, sh as usize, sigma / f as f32);
    resize::gray(&small, sw, sh, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_sum_to_expected_variance() {
        for sigma in [1.0f32, 3.0, 8.0, 20.0] {
            let radii = boxes_for_gauss(sigma, 3);
            // Variance of a box of width w is (w^2 - 1) / 12.
            let var: f32 = radii
                .iter()
                .map(|&r| {
                    let w = (2 * r + 1) as f32;
                    (w * w - 1.0) / 12.0
                })
                .sum();
            assert!(
                (var.sqrt() - sigma).abs() < sigma * 0.25 + 0.5,
                "sigma {sigma} -> {var}"
            );
        }
    }

    #[test]
    fn flat_image_is_unchanged() {
        let src = [40u8, 80, 120, 255].repeat(64 * 48);
        let out = rgba(&src, 64, 48, 6.0).unwrap();
        assert!(out.chunks(4).all(|p| p == [40, 80, 120, 255]));
    }

    #[test]
    fn blur_spreads_an_impulse() {
        let (w, h) = (21usize, 21usize);
        let mut img = vec![0u8; w * h];
        img[10 * w + 10] = 255;
        gaussian_in_place::<1>(&mut img, w, h, 2.0);
        assert!(img[10 * w + 10] < 255);
        assert!(img[10 * w + 11] > 0);
        assert!(img[11 * w + 10] > 0);
        // Symmetric.
        assert_eq!(img[10 * w + 9], img[10 * w + 11]);
        assert_eq!(img[9 * w + 10], img[11 * w + 10]);
    }

    #[test]
    fn non_square_transpose_roundtrip() {
        let (w, h) = (5usize, 3usize);
        let src: Vec<u8> = (0..(w * h * 4) as u32).map(|v| v as u8).collect();
        let mut t = vec![0u8; src.len()];
        let mut back = vec![0u8; src.len()];
        transpose::<4>(&src, &mut t, w, h);
        transpose::<4>(&t, &mut back, h, w);
        assert_eq!(src, back);
    }
}
