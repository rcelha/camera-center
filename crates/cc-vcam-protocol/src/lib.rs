//! Contract between the Camera Center app (writer) and the Windows Media
//! Foundation virtual camera source (reader), which runs inside the Frame
//! Server service.
//!
//! The source creates a named file mapping of [`MAPPING_SIZE`] bytes laid out
//! as a [`HEADER_SIZE`]-byte header followed by one NV12 frame. Writes are
//! guarded by a sequence lock: the writer makes `seq` odd, copies the frame,
//! then makes it even again; readers retry if `seq` changed or was odd.

#![no_std]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// CLSID of the media source COM class: {6A1C8E2B-5F4D-4B3A-9C7E-1D2F3A4B5C6D}.
pub const SOURCE_CLSID_U128: u128 = 0x6a1c8e2b_5f4d_4b3a_9c7e_1d2f3a4b5c6d;
pub const SOURCE_CLSID_STR: &str = "{6A1C8E2B-5F4D-4B3A-9C7E-1D2F3A4B5C6D}";
/// Friendly name shown by video apps.
pub const DEVICE_NAME: &str = "Camera Center";
/// Global namespace so the app (user session) and Frame Server (session 0) meet.
pub const MAPPING_NAME: &str = "Global\\CameraCenterVirtualCamera";

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
pub const FPS: u32 = 30;
pub const FRAME_SIZE: usize = (WIDTH * HEIGHT * 3 / 2) as usize;
pub const HEADER_SIZE: usize = 64;
pub const MAPPING_SIZE: usize = HEADER_SIZE + FRAME_SIZE;

pub const MAGIC: u32 = u32::from_le_bytes(*b"CCVC");
pub const VERSION: u32 = 1;

/// Header at the start of the mapping. All fields are accessed atomically.
#[repr(C)]
pub struct Header {
    pub magic: AtomicU32,
    pub version: AtomicU32,
    pub width: AtomicU32,
    pub height: AtomicU32,
    /// Sequence lock; odd while a write is in progress.
    pub seq: AtomicU64,
    /// Writer timestamp in 100ns units (any monotonic clock).
    pub timestamp: AtomicU64,
}

const _: () = assert!(core::mem::size_of::<Header>() <= HEADER_SIZE);

/// View over a mapping of at least [`MAPPING_SIZE`] bytes.
pub struct SharedFrame {
    base: *mut u8,
}

impl SharedFrame {
    /// # Safety
    /// `base` must point to at least [`MAPPING_SIZE`] writable bytes, aligned
    /// to 8 bytes, that stay mapped for the lifetime of the returned value.
    pub unsafe fn new(base: *mut u8) -> Self {
        Self { base }
    }

    fn header(&self) -> &Header {
        // SAFETY: guaranteed by `new`; Header only contains atomics.
        unsafe { &*(self.base as *const Header) }
    }

    fn frame_ptr(&self) -> *mut u8 {
        // SAFETY: within the mapping per `new`.
        unsafe { self.base.add(HEADER_SIZE) }
    }

    /// Initialises the header (done by the creator of the mapping).
    pub fn init(&self) {
        let h = self.header();
        h.width.store(WIDTH, Ordering::Relaxed);
        h.height.store(HEIGHT, Ordering::Relaxed);
        h.version.store(VERSION, Ordering::Relaxed);
        h.seq.store(0, Ordering::Relaxed);
        h.timestamp.store(0, Ordering::Relaxed);
        h.magic.store(MAGIC, Ordering::Release);
    }

    pub fn is_valid(&self) -> bool {
        let h = self.header();
        h.magic.load(Ordering::Acquire) == MAGIC
            && h.version.load(Ordering::Relaxed) == VERSION
            && h.width.load(Ordering::Relaxed) == WIDTH
            && h.height.load(Ordering::Relaxed) == HEIGHT
    }

    /// Publishes one NV12 frame. Returns `false` if `nv12` has the wrong size.
    pub fn write(&self, nv12: &[u8], timestamp: u64) -> bool {
        if nv12.len() != FRAME_SIZE {
            return false;
        }
        let h = self.header();
        let seq = h.seq.load(Ordering::Relaxed);
        let odd = seq | 1;
        h.seq.store(odd, Ordering::Relaxed);
        fence(Ordering::Release);
        // SAFETY: the frame region is FRAME_SIZE bytes (see `new`).
        unsafe { core::ptr::copy_nonoverlapping(nv12.as_ptr(), self.frame_ptr(), FRAME_SIZE) };
        h.timestamp.store(timestamp, Ordering::Relaxed);
        h.seq.store(odd.wrapping_add(1), Ordering::Release);
        true
    }

    /// Copies the latest complete frame into `out` (FRAME_SIZE bytes).
    /// Returns `(seq, timestamp)` or `None` if no frame was written yet or a
    /// consistent copy couldn't be made after a few attempts.
    pub fn read(&self, out: &mut [u8]) -> Option<(u64, u64)> {
        if out.len() != FRAME_SIZE || !self.is_valid() {
            return None;
        }
        let h = self.header();
        for _ in 0..4 {
            let before = h.seq.load(Ordering::Acquire);
            if before == 0 {
                return None;
            }
            if before & 1 == 1 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: FRAME_SIZE bytes on both sides; torn reads are detected
            // by the sequence check below and discarded.
            unsafe {
                core::ptr::copy_nonoverlapping(self.frame_ptr(), out.as_mut_ptr(), FRAME_SIZE)
            };
            fence(Ordering::Acquire);
            let ts = h.timestamp.load(Ordering::Relaxed);
            if h.seq.load(Ordering::Relaxed) == before {
                return Some((before, ts));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    use super::*;

    fn mapping() -> Vec<u64> {
        vec![0u64; MAPPING_SIZE.div_ceil(8)]
    }

    #[test]
    fn write_then_read() {
        let mut mem = mapping();
        let shm = unsafe { SharedFrame::new(mem.as_mut_ptr().cast()) };
        let mut out = vec![0u8; FRAME_SIZE];
        assert!(shm.read(&mut out).is_none(), "uninitialised");
        shm.init();
        assert!(shm.is_valid());
        assert!(shm.read(&mut out).is_none(), "no frame yet");

        let frame = vec![7u8; FRAME_SIZE];
        assert!(shm.write(&frame, 42));
        let (seq, ts) = shm.read(&mut out).unwrap();
        assert_eq!((seq, ts), (2, 42));
        assert_eq!(out, frame);

        assert!(shm.write(&vec![9u8; FRAME_SIZE], 43));
        assert_eq!(shm.read(&mut out).unwrap().0, 4);
        assert!(out.iter().all(|&b| b == 9));
    }

    #[test]
    fn rejects_wrong_sizes() {
        let mut mem = mapping();
        let shm = unsafe { SharedFrame::new(mem.as_mut_ptr().cast()) };
        shm.init();
        assert!(!shm.write(&[0u8; 10], 0));
        assert!(shm.read(&mut [0u8; 10]).is_none());
    }

    #[test]
    fn in_progress_write_is_not_read() {
        let mut mem = mapping();
        let shm = unsafe { SharedFrame::new(mem.as_mut_ptr().cast()) };
        shm.init();
        shm.write(&vec![1u8; FRAME_SIZE], 1);
        shm.header().seq.store(3, Ordering::Relaxed); // writer mid-way
        assert!(shm.read(&mut vec![0u8; FRAME_SIZE]).is_none());
    }

    #[test]
    fn clsid_string_matches() {
        let s = std::format!("{:032X}", SOURCE_CLSID_U128);
        let expected = std::format!(
            "{{{}-{}-{}-{}-{}}}",
            &s[0..8],
            &s[8..12],
            &s[12..16],
            &s[16..20],
            &s[20..32]
        );
        assert_eq!(expected, SOURCE_CLSID_STR);
    }
}
