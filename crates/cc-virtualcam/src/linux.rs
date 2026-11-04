//! v4l2loopback output.
//!
//! The kernel module must be loaded by the user, e.g.
//! `sudo modprobe v4l2loopback exclusive_caps=1 card_label="Camera Center"`.
//! We look for a loopback node whose card name matches [`DEVICE_NAME`]
//! (falling back to any loopback device), set a YUYV output format and
//! `write()` frames to it.

use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use cc_core::{Frame, convert};
use tracing::info;

use crate::{DEVICE_NAME, OUTPUT_HEIGHT, OUTPUT_WIDTH, SinkStatus, VirtualCamera, to_output_size};

const LOOPBACK_DRIVER: &str = "v4l2 loopback";
const V4L2_CAP_VIDEO_OUTPUT: u32 = 0x0000_0002;
const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const V4L2_FIELD_NONE: u32 = 1;
const V4L2_COLORSPACE_SMPTE170M: u32 = 1;
const V4L2_PIX_FMT_YUYV: u32 = u32::from_le_bytes(*b"YUYV");

#[repr(C)]
struct V4l2Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

/// `struct v4l2_format`: the union is 200 bytes and 8-byte aligned on 64-bit.
#[repr(C, align(8))]
struct V4l2Format {
    type_: u32,
    _pad: u32,
    pix: V4l2PixFormat,
    _rest: [u8; 200 - std::mem::size_of::<V4l2PixFormat>()],
}

const _: () = assert!(std::mem::size_of::<V4l2Capability>() == 104);
const _: () = assert!(std::mem::size_of::<V4l2Format>() == 208);

// _IOR('V', 0, struct v4l2_capability) and _IOWR('V', 5, struct v4l2_format)
const VIDIOC_QUERYCAP: u64 = 0x8068_5600;
const VIDIOC_S_FMT: u64 = 0xC0D0_5605;

fn cstr(bytes: &[u8]) -> String {
    CStr::from_bytes_until_nul(bytes)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn query_cap(file: &File) -> Option<V4l2Capability> {
    // SAFETY: V4l2Capability is plain old data, zero is a valid bit pattern,
    // and the kernel writes at most size_of::<V4l2Capability>() bytes.
    let mut cap: V4l2Capability = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), VIDIOC_QUERYCAP as _, &mut cap) };
    (rc == 0).then_some(cap)
}

/// Finds loopback output nodes as (path, card name).
fn find_loopback_devices() -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return found;
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("video"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
            continue;
        };
        let Some(cap) = query_cap(&file) else {
            continue;
        };
        let caps = if cap.capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        if cstr(&cap.driver) == LOOPBACK_DRIVER && caps & V4L2_CAP_VIDEO_OUTPUT != 0 {
            found.push((path, cstr(&cap.card)));
        }
    }
    found
}

pub struct V4l2Loopback {
    file: File,
    path: PathBuf,
    card: String,
    yuyv: Vec<u8>,
    streaming: bool,
}

impl V4l2Loopback {
    pub fn open() -> std::result::Result<Self, SinkStatus> {
        let devices = find_loopback_devices();
        let Some((path, card)) = devices
            .iter()
            .find(|(_, card)| card == DEVICE_NAME)
            .or_else(|| devices.first())
            .cloned()
        else {
            return Err(SinkStatus::NotInstalled {
                hint: format!(
                    "Load v4l2loopback: sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"{DEVICE_NAME}\""
                ),
            });
        };
        Self::open_path(path, card).map_err(|e| SinkStatus::Error(format!("{e:#}")))
    }

    fn open_path(path: PathBuf, card: String) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        let (w, h) = (OUTPUT_WIDTH, OUTPUT_HEIGHT);
        // SAFETY: zero is a valid bit pattern for this POD struct.
        let mut fmt: V4l2Format = unsafe { std::mem::zeroed() };
        fmt.type_ = V4L2_BUF_TYPE_VIDEO_OUTPUT;
        fmt.pix = V4l2PixFormat {
            width: w,
            height: h,
            pixelformat: V4L2_PIX_FMT_YUYV,
            field: V4L2_FIELD_NONE,
            bytesperline: w * 2,
            sizeimage: w * h * 2,
            colorspace: V4L2_COLORSPACE_SMPTE170M,
            priv_: 0,
            flags: 0,
            ycbcr_enc: 0,
            quantization: 0,
            xfer_func: 0,
        };
        // SAFETY: fmt is a correctly sized v4l2_format living for the call.
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), VIDIOC_S_FMT as _, &mut fmt) };
        if rc != 0 {
            bail!(
                "VIDIOC_S_FMT on {} failed: {}",
                path.display(),
                std::io::Error::last_os_error()
            );
        }
        info!(device = %path.display(), %card, "v4l2loopback output ready");
        Ok(Self {
            file,
            path,
            card,
            yuyv: vec![0; (w * h * 2) as usize],
            streaming: false,
        })
    }
}

impl VirtualCamera for V4l2Loopback {
    fn name(&self) -> String {
        format!("{} ({})", self.card, self.path.display())
    }

    fn status(&self) -> SinkStatus {
        if self.streaming {
            SinkStatus::Streaming
        } else {
            SinkStatus::Ready
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<()> {
        let rgba = to_output_size(frame)?;
        convert::rgba_to_yuyv(&rgba, OUTPUT_WIDTH, OUTPUT_HEIGHT, &mut self.yuyv)?;
        self.file
            .write_all(&self.yuyv)
            .with_context(|| format!("write to {} failed", self.path.display()))?;
        self.streaming = true;
        Ok(())
    }
}
