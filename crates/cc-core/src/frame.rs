//! Frame and mask containers.

use anyhow::{Result, ensure};

use crate::resize;

/// A tightly packed RGBA8 video frame.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, no padding.
    pub data: Vec<u8>,
    /// Monotonic sequence number assigned by the capture stage.
    pub seq: u64,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl Frame {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        ensure!(width > 0 && height > 0, "frame dimensions must be non-zero");
        ensure!(
            data.len() == pixel_count(width, height) * 4,
            "frame buffer is {} bytes, expected {} for {width}x{height} RGBA",
            data.len(),
            pixel_count(width, height) * 4
        );
        Ok(Self {
            width,
            height,
            data,
            seq: 0,
        })
    }

    /// A frame filled with a single colour.
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let data = rgba.repeat(pixel_count(width, height));
        Self {
            width,
            height,
            data,
            seq: 0,
        }
    }

    pub fn with_seq(mut self, seq: u64) -> Self {
        self.seq = seq;
        self
    }

    pub fn pixel_count(&self) -> usize {
        pixel_count(self.width, self.height)
    }

    /// Row stride in bytes.
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    /// Returns a resized copy (bilinear).
    pub fn resized(&self, width: u32, height: u32) -> Result<Frame> {
        if width == self.width && height == self.height {
            return Ok(self.clone());
        }
        let data = resize::rgba(&self.data, self.width, self.height, width, height)?;
        Ok(Frame {
            width,
            height,
            data,
            seq: self.seq,
        })
    }

    /// Scales the frame down (never up) so that its height is at most
    /// `max_height`, preserving aspect ratio. Width is kept even so the frame
    /// can be converted to 4:2:x formats.
    pub fn fit_height(self, max_height: Option<u32>) -> Result<Frame> {
        let Some(max_h) = max_height else {
            return Ok(self);
        };
        if self.height <= max_h {
            return Ok(self);
        }
        let (w, h) = scaled_to_height(self.width, self.height, max_h);
        self.resized(w, h)
    }

    /// Returns a copy scaled to fit inside `max_w`x`max_h` (aspect preserved).
    pub fn thumbnail(&self, max_w: u32, max_h: u32) -> Result<Frame> {
        let scale = (max_w as f64 / self.width as f64)
            .min(max_h as f64 / self.height as f64)
            .min(1.0);
        let w = ((self.width as f64 * scale).round() as u32).max(1);
        let h = ((self.height as f64 * scale).round() as u32).max(1);
        self.resized(w, h)
    }
}

/// Soft foreground mask: 255 = person, 0 = background.
#[derive(Clone, PartialEq, Eq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Mask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mask")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Mask {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        ensure!(width > 0 && height > 0, "mask dimensions must be non-zero");
        ensure!(
            data.len() == pixel_count(width, height),
            "mask buffer is {} bytes, expected {}",
            data.len(),
            pixel_count(width, height)
        );
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub fn filled(width: u32, height: u32, value: u8) -> Self {
        Self {
            width,
            height,
            data: vec![value; pixel_count(width, height)],
        }
    }

    /// Returns the mask resized to `width`x`height` (bilinear).
    pub fn resized(&self, width: u32, height: u32) -> Result<Mask> {
        if width == self.width && height == self.height {
            return Ok(self.clone());
        }
        let data = resize::gray(&self.data, self.width, self.height, width, height)?;
        Ok(Mask {
            width,
            height,
            data,
        })
    }
}

pub(crate) fn pixel_count(width: u32, height: u32) -> usize {
    width as usize * height as usize
}

/// Dimensions scaled to `target_h`, with an even width.
pub fn scaled_to_height(width: u32, height: u32, target_h: u32) -> (u32, u32) {
    let w = (width as f64 * target_h as f64 / height as f64).round() as u32;
    ((w & !1).max(2), target_h.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_validates_length() {
        assert!(Frame::new(2, 2, vec![0; 16]).is_ok());
        assert!(Frame::new(2, 2, vec![0; 15]).is_err());
        assert!(Frame::new(0, 2, vec![]).is_err());
        assert!(Mask::new(2, 2, vec![0; 4]).is_ok());
        assert!(Mask::new(2, 2, vec![0; 5]).is_err());
    }

    #[test]
    fn fit_height_only_downscales() {
        let f = Frame::filled(1920, 1080, [1, 2, 3, 255]);
        let f720 = f.clone().fit_height(Some(720)).unwrap();
        assert_eq!((f720.width, f720.height), (1280, 720));
        let same = f.clone().fit_height(Some(2160)).unwrap();
        assert_eq!((same.width, same.height), (1920, 1080));
        let native = f.fit_height(None).unwrap();
        assert_eq!(native.height, 1080);
    }

    #[test]
    fn resize_preserves_flat_colour() {
        let f = Frame::filled(64, 48, [10, 20, 30, 255]);
        let r = f.resized(32, 24).unwrap();
        assert!(r.data.chunks(4).all(|p| p == [10, 20, 30, 255]));
        let m = Mask::filled(10, 10, 200).resized(40, 40).unwrap();
        assert!(m.data.iter().all(|&v| v == 200));
    }

    #[test]
    fn thumbnail_keeps_aspect() {
        let f = Frame::filled(1920, 1080, [0, 0, 0, 255]);
        let t = f.thumbnail(640, 640).unwrap();
        assert_eq!((t.width, t.height), (640, 360));
    }
}
