//! Pixel format conversion between the internal RGBA8 format and the formats
//! cameras / virtual camera APIs use. YUV conversions use BT.601 limited range,
//! which is what UVC webcams and conferencing apps expect.

use anyhow::{Result, ensure};
use rayon::prelude::*;

/// Packed RGB24 → RGBA8 (alpha = 255).
pub fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for p in rgb.chunks_exact(3) {
        out.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    out
}

/// RGBA8 → packed RGB24.
pub fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for p in rgba.chunks_exact(4) {
        out.extend_from_slice(&p[..3]);
    }
    out
}

/// RGBA8 → BGRA8 into `out` (same length).
pub fn rgba_to_bgra(rgba: &[u8], out: &mut [u8]) {
    out.par_chunks_mut(4 * 1024)
        .zip(rgba.par_chunks(4 * 1024))
        .for_each(|(o, i)| {
            for (o, i) in o.chunks_exact_mut(4).zip(i.chunks_exact(4)) {
                o[0] = i[2];
                o[1] = i[1];
                o[2] = i[0];
                o[3] = i[3];
            }
        });
}

#[inline]
fn rgb_to_y(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8
}

#[inline]
fn rgb_to_uv(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

#[inline]
fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    [
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    ]
}

fn px(rgba: &[u8], i: usize) -> (i32, i32, i32) {
    (rgba[i] as i32, rgba[i + 1] as i32, rgba[i + 2] as i32)
}

/// RGBA8 → YUYV (YUY2) 4:2:2. `width` must be even.
pub fn rgba_to_yuyv(rgba: &[u8], width: u32, height: u32, out: &mut [u8]) -> Result<()> {
    let (w, h) = (width as usize, height as usize);
    ensure!(w % 2 == 0, "YUYV requires an even width");
    ensure!(
        rgba.len() == w * h * 4 && out.len() == w * h * 2,
        "buffer size mismatch"
    );
    out.par_chunks_mut(w * 2)
        .zip(rgba.par_chunks(w * 4))
        .for_each(|(o, row)| {
            for x in (0..w).step_by(2) {
                let (r0, g0, b0) = px(row, x * 4);
                let (r1, g1, b1) = px(row, x * 4 + 4);
                let (u, v) = rgb_to_uv((r0 + r1) / 2, (g0 + g1) / 2, (b0 + b1) / 2);
                o[x * 2] = rgb_to_y(r0, g0, b0);
                o[x * 2 + 1] = u;
                o[x * 2 + 2] = rgb_to_y(r1, g1, b1);
                o[x * 2 + 3] = v;
            }
        });
    Ok(())
}

/// YUYV → RGBA8.
pub fn yuyv_to_rgba(yuyv: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    ensure!(
        w % 2 == 0 && yuyv.len() == w * h * 2,
        "buffer size mismatch"
    );
    let mut out = vec![0u8; w * h * 4];
    for (o, i) in out.chunks_exact_mut(8).zip(yuyv.chunks_exact(4)) {
        let a = yuv_to_rgb(i[0], i[1], i[3]);
        let b = yuv_to_rgb(i[2], i[1], i[3]);
        o[..3].copy_from_slice(&a);
        o[3] = 255;
        o[4..7].copy_from_slice(&b);
        o[7] = 255;
    }
    Ok(out)
}

/// RGBA8 → NV12 (Y plane followed by interleaved UV at quarter resolution).
/// Width and height must be even.
pub fn rgba_to_nv12(rgba: &[u8], width: u32, height: u32, out: &mut [u8]) -> Result<()> {
    let (w, h) = (width as usize, height as usize);
    ensure!(w % 2 == 0 && h % 2 == 0, "NV12 requires even dimensions");
    ensure!(
        rgba.len() == w * h * 4 && out.len() == w * h * 3 / 2,
        "buffer size mismatch"
    );
    let (y_plane, uv_plane) = out.split_at_mut(w * h);
    y_plane
        .par_chunks_mut(w)
        .zip(rgba.par_chunks(w * 4))
        .for_each(|(o, row)| {
            for (y, p) in o.iter_mut().zip(row.chunks_exact(4)) {
                *y = rgb_to_y(p[0] as i32, p[1] as i32, p[2] as i32);
            }
        });
    uv_plane.par_chunks_mut(w).enumerate().for_each(|(cy, o)| {
        let r0 = &rgba[(cy * 2) * w * 4..(cy * 2 + 1) * w * 4];
        let r1 = &rgba[(cy * 2 + 1) * w * 4..(cy * 2 + 2) * w * 4];
        for cx in 0..w / 2 {
            let mut acc = (0, 0, 0);
            for row in [r0, r1] {
                for dx in 0..2 {
                    let (r, g, b) = px(row, (cx * 2 + dx) * 4);
                    acc = (acc.0 + r, acc.1 + g, acc.2 + b);
                }
            }
            let (u, v) = rgb_to_uv(acc.0 / 4, acc.1 / 4, acc.2 / 4);
            o[cx * 2] = u;
            o[cx * 2 + 1] = v;
        }
    });
    Ok(())
}

/// NV12 → RGBA8.
pub fn nv12_to_rgba(nv12: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    ensure!(nv12.len() == w * h * 3 / 2, "buffer size mismatch");
    let (y_plane, uv) = nv12.split_at(w * h);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let ci = (y / 2) * w + (x / 2) * 2;
            let rgb = yuv_to_rgb(y_plane[y * w + x], uv[ci], uv[ci + 1]);
            let o = (y * w + x) * 4;
            out[o..o + 3].copy_from_slice(&rgb);
            out[o + 3] = 255;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image(w: usize, h: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                // Smooth gradients survive chroma subsampling.
                v.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, 128, 255]);
            }
        }
        v
    }

    fn max_diff(a: &[u8], b: &[u8]) -> u8 {
        a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap()
    }

    #[test]
    fn bgra_swaps_channels() {
        let src = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut out = [0u8; 8];
        rgba_to_bgra(&src, &mut out);
        assert_eq!(out, [3, 2, 1, 4, 7, 6, 5, 8]);
    }

    #[test]
    fn rgb_roundtrip() {
        let rgb = [1, 2, 3, 4, 5, 6];
        let rgba = rgb_to_rgba(&rgb);
        assert_eq!(rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(rgba_to_rgb(&rgba), rgb);
    }

    #[test]
    fn yuyv_roundtrip_is_close() {
        let (w, h) = (64, 32);
        let src = test_image(w, h);
        let mut yuyv = vec![0u8; w * h * 2];
        rgba_to_yuyv(&src, w as u32, h as u32, &mut yuyv).unwrap();
        let back = yuyv_to_rgba(&yuyv, w as u32, h as u32).unwrap();
        assert!(max_diff(&src, &back) <= 8, "diff {}", max_diff(&src, &back));
    }

    #[test]
    fn nv12_roundtrip_is_close() {
        let (w, h) = (64, 32);
        let src = test_image(w, h);
        let mut nv12 = vec![0u8; w * h * 3 / 2];
        rgba_to_nv12(&src, w as u32, h as u32, &mut nv12).unwrap();
        let back = nv12_to_rgba(&nv12, w as u32, h as u32).unwrap();
        assert!(
            max_diff(&src, &back) <= 10,
            "diff {}",
            max_diff(&src, &back)
        );
    }

    #[test]
    fn known_colours() {
        let mut out = [0u8; 4];
        // White → Y=235, black → Y=16, neutral chroma.
        rgba_to_yuyv(&[255, 255, 255, 255, 0, 0, 0, 255], 2, 1, &mut out).unwrap();
        assert_eq!(out[0], 235);
        assert_eq!(out[2], 16);
        assert_eq!(out[1], 128);
        assert_eq!(out[3], 128);
    }

    #[test]
    fn rejects_odd_sizes() {
        let mut out = vec![0u8; 6];
        assert!(rgba_to_yuyv(&[0; 12], 3, 1, &mut out).is_err());
        assert!(rgba_to_nv12(&[0; 16], 2, 2, &mut out[..5]).is_err());
    }
}
