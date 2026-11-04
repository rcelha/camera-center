//! Thin wrappers over `fast_image_resize` for the pixel layouts we use.

use std::cell::RefCell;

use anyhow::{Context, Result};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};

thread_local! {
    static RESIZER: RefCell<Resizer> = RefCell::new(Resizer::new());
}

fn run(
    src: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    pixel_type: PixelType,
    options: &ResizeOptions,
) -> Result<Vec<u8>> {
    let src = ImageRef::new(sw, sh, src, pixel_type).context("invalid source image")?;
    let mut dst = Image::new(dw, dh, pixel_type);
    RESIZER.with(|r| r.borrow_mut().resize(&src, &mut dst, options))?;
    Ok(dst.into_vec())
}

fn options(filter: FilterType) -> ResizeOptions {
    // Camera frames are opaque, skip alpha premultiplication.
    ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(filter))
        .use_alpha(false)
}

/// Bilinear RGBA resize.
pub fn rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Vec<u8>> {
    run(
        src,
        sw,
        sh,
        dw,
        dh,
        PixelType::U8x4,
        &options(FilterType::Bilinear),
    )
}

/// Box-filtered RGBA downscale, good for large reductions.
pub fn rgba_box(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Vec<u8>> {
    run(
        src,
        sw,
        sh,
        dw,
        dh,
        PixelType::U8x4,
        &options(FilterType::Box),
    )
}

/// RGBA resize that fills the destination, cropping the source to match the
/// destination aspect ratio ("cover" fit, centred).
pub fn rgba_cover(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Vec<u8>> {
    let opts = options(FilterType::CatmullRom).fit_into_destination(Some((0.5, 0.5)));
    run(src, sw, sh, dw, dh, PixelType::U8x4, &opts)
}

/// RGBA resize that fits the whole source inside the destination, centred,
/// with black bars ("contain" / letterbox).
pub fn rgba_letterbox(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Vec<u8>> {
    if (sw, sh) == (dw, dh) {
        return Ok(src.to_vec());
    }
    let scale = (dw as f64 / sw as f64).min(dh as f64 / sh as f64);
    let fw = ((sw as f64 * scale).round() as u32).clamp(1, dw);
    let fh = ((sh as f64 * scale).round() as u32).clamp(1, dh);
    let fitted = rgba(src, sw, sh, fw, fh)?;
    if (fw, fh) == (dw, dh) {
        return Ok(fitted);
    }
    let mut out = [0u8, 0, 0, 255].repeat(dw as usize * dh as usize);
    let (ox, oy) = ((dw - fw) as usize / 2, (dh - fh) as usize / 2);
    for (y, row) in fitted.chunks_exact(fw as usize * 4).enumerate() {
        let start = ((oy + y) * dw as usize + ox) * 4;
        out[start..start + row.len()].copy_from_slice(row);
    }
    Ok(out)
}

/// Bilinear single-channel resize.
pub fn gray(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Vec<u8>> {
    run(
        src,
        sw,
        sh,
        dw,
        dh,
        PixelType::U8,
        &options(FilterType::Bilinear),
    )
}

/// RGB (3 channel) resize, used to feed models.
pub fn rgb(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Vec<u8>> {
    run(
        src,
        sw,
        sh,
        dw,
        dh,
        PixelType::U8x3,
        &options(FilterType::Bilinear),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_fills_destination() {
        // 4x2 image: left half red, right half blue. Cover into 2x2 keeps the centre.
        let mut src = Vec::new();
        for _y in 0..2 {
            src.extend_from_slice(&[255, 0, 0, 255, 255, 0, 0, 255]);
            src.extend_from_slice(&[0, 0, 255, 255, 0, 0, 255, 255]);
        }
        let out = rgba_cover(&src, 4, 2, 2, 2).unwrap();
        assert_eq!(out.len(), 16);
    }

    #[test]
    fn letterbox_adds_bars() {
        // 2x2 white into 4x2 → white centre, black sides.
        let src = [255u8; 16];
        let out = rgba_letterbox(&src, 2, 2, 4, 2).unwrap();
        assert_eq!(&out[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out[4..8], &[255, 255, 255, 255]);
        assert_eq!(&out[12..16], &[0, 0, 0, 255]);
    }

    #[test]
    fn rejects_short_buffers() {
        assert!(rgba(&[0; 10], 2, 2, 1, 1).is_err());
    }
}
