//! v4l2loopback output.
//!
//! The kernel module is loaded from the app with [`load_module`] (through
//! pkexec), or by hand:
//! `sudo modprobe v4l2loopback exclusive_caps=1 card_label="Camera Center"`.
//! [`LoopbackSink`] keeps looking for the device, so it connects as soon as
//! the module is loaded. We look for a loopback node whose card name matches [`DEVICE_NAME`]
//! (falling back to any loopback device), set a YUYV output format and
//! `write()` frames to it.

use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cc_core::{Frame, convert};
use tracing::{debug, info, warn};

use crate::{DEVICE_NAME, OUTPUT_HEIGHT, OUTPUT_WIDTH, SinkStatus, VirtualCamera, to_output_size};

const LOOPBACK_DRIVER: &str = "v4l2 loopback";
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
// _IOW('V', 18, int)
const VIDIOC_STREAMON: u64 = 0x4004_5612;

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
        // Match on the driver only: with exclusive_caps=1, v4l2loopback stops
        // advertising VIDEO_OUTPUT after a previous writer closes the device,
        // yet it still accepts a new writer.
        if cstr(&cap.driver) == LOOPBACK_DRIVER {
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
                hint: not_installed_hint(),
            });
        };
        Self::open_path(path, card).map_err(|e| {
            SinkStatus::Error(if can_load_module() {
                format!("{e:#}. Click Install virtual camera to reset the device")
            } else {
                format!("{e:#}")
            })
        })
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
        // v4l2loopback < 0.13 only registers a producer as the writer on
        // STREAMON; without it, closing a write()-based producer leaves an
        // exclusive_caps device refusing every new producer until the module
        // is reloaded. Newer versions don't need it, so failures are ignored.
        let mut buf_type = V4L2_BUF_TYPE_VIDEO_OUTPUT as libc::c_int;
        // SAFETY: STREAMON reads one int, which lives for the call.
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), VIDIOC_STREAMON as _, &mut buf_type) };
        if rc != 0 {
            debug!(
                "VIDIOC_STREAMON on {} failed: {}",
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

const PKEXEC_PATHS: [&str; 2] = ["/usr/bin/pkexec", "/bin/pkexec"];
/// How often [`LoopbackSink`] looks for the loopback device while it's missing.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// Exit code of [`load_script`] when the v4l2loopback module isn't installed.
const EXIT_MODULE_MISSING: i32 = 3;

/// Shell script run as root through pkexec: (re)loads v4l2loopback now and on
/// every boot, with the device labelled [`DEVICE_NAME`]. Reloading resets a
/// device left unusable by an older producer; it's skipped while in use.
fn load_script() -> String {
    let options = format!("exclusive_caps=1 card_label=\"{DEVICE_NAME}\"");
    format!(
        "set -e\n\
         modprobe -n -q v4l2loopback || exit {EXIT_MODULE_MISSING}\n\
         modprobe -r -q v4l2loopback || true\n\
         modprobe v4l2loopback {options}\n\
         printf '%s\\n' 'options v4l2loopback {options}' > /etc/modprobe.d/camera-center.conf\n\
         printf '%s\\n' v4l2loopback > /etc/modules-load.d/camera-center.conf\n"
    )
}

/// Progress of the most recent [`load_module`] request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleState {
    Idle,
    Loading,
    Failed(String),
}

static MODULE_STATE: Mutex<ModuleState> = Mutex::new(ModuleState::Idle);

fn set_module_state(state: ModuleState) {
    info!(?state, "v4l2loopback module");
    *MODULE_STATE.lock().unwrap_or_else(|e| e.into_inner()) = state;
}

pub fn module_state() -> ModuleState {
    MODULE_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn pkexec() -> Option<&'static Path> {
    PKEXEC_PATHS.iter().map(Path::new).find(|p| p.exists())
}

/// Whether [`load_module`] can work here (needs polkit's pkexec).
pub fn can_load_module() -> bool {
    pkexec().is_some()
}

/// Loads v4l2loopback in the background, asking for the admin password with
/// the desktop's polkit dialog. Progress is reported through [`module_state`];
/// the sink picks the device up on its own once the module is loaded.
pub fn load_module() {
    {
        let mut state = MODULE_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if *state == ModuleState::Loading {
            return;
        }
        *state = ModuleState::Loading;
    }
    info!("loading v4l2loopback through pkexec");
    std::thread::spawn(|| {
        let state = match run_load_script() {
            Ok(()) => ModuleState::Idle,
            Err(e) => {
                warn!("loading v4l2loopback failed: {e}");
                ModuleState::Failed(e)
            }
        };
        set_module_state(state);
    });
}

fn has_loopback_module(modules_dir: &Path) -> bool {
    fn walk(dir: &Path, depth: u8) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|e| {
            let path = e.path();
            if path.is_dir() {
                depth > 0 && walk(&path, depth - 1)
            } else {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("v4l2loopback.ko"))
            }
        })
    }
    walk(modules_dir, 6)
}

/// Explains a missing module before asking for a password. The common case is
/// DKMS having built it only for a newer, installed but not yet booted kernel.
fn check_module_for_running_kernel() -> std::result::Result<(), String> {
    let Ok(release) = std::fs::read_to_string("/proc/sys/kernel/osrelease") else {
        return Ok(());
    };
    let release = release.trim();
    let root = Path::new("/lib/modules");
    if has_loopback_module(&root.join(release)) {
        return Ok(());
    }
    let built_for: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| has_loopback_module(&e.path()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    if built_for.is_empty() {
        return Ok(()); // let modprobe report it (the module may be built in)
    }
    Err(format!(
        "v4l2loopback is installed for kernel {} but the running kernel is {release}. \
         Restart your computer to boot the newer kernel, or install linux-headers-{release} \
         so DKMS can build the module for it",
        built_for.join(", ")
    ))
}

fn run_load_script() -> std::result::Result<(), String> {
    let pkexec = pkexec().ok_or("pkexec is not available")?;
    check_module_for_running_kernel()?;
    // AppImage runtimes export library paths that must not leak into system tools.
    let output = Command::new(pkexec)
        .args(["/bin/sh", "-c", &load_script()])
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("LD_PRELOAD")
        .output()
        .map_err(|e| format!("cannot run pkexec: {e}"))?;
    match output.status.code() {
        Some(0) => Ok(()),
        Some(EXIT_MODULE_MISSING) => Err("the v4l2loopback module isn't installed. Install it \
             with your package manager (e.g. sudo apt install v4l2loopback-dkms) and try again"
            .into()),
        // pkexec: 126 = dialog dismissed, 127 = not authorized.
        Some(126 | 127) => Err("authorization was cancelled or denied".into()),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(match stderr.trim() {
                "" => format!("modprobe failed ({})", output.status),
                msg => msg.to_string(),
            })
        }
    }
}

fn not_installed_hint() -> String {
    match module_state() {
        ModuleState::Loading => "Loading the v4l2loopback module…".into(),
        ModuleState::Failed(e) => format!("Could not load v4l2loopback: {e}"),
        ModuleState::Idle if can_load_module() => {
            "Click Install virtual camera to load the v4l2loopback module".into()
        }
        ModuleState::Idle => format!(
            "Load v4l2loopback: sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"{DEVICE_NAME}\""
        ),
    }
}

/// The platform virtual camera: a [`V4l2Loopback`] opened on demand, so it
/// connects once the module is loaded and reconnects if it goes away.
pub struct LoopbackSink {
    camera: Option<V4l2Loopback>,
    error: Option<SinkStatus>,
    last_attempt: Option<Instant>,
}

impl LoopbackSink {
    pub fn new() -> Self {
        let mut sink = Self {
            camera: None,
            error: None,
            last_attempt: None,
        };
        sink.try_open();
        sink
    }

    fn try_open(&mut self) {
        if self.camera.is_some()
            || self
                .last_attempt
                .is_some_and(|t| t.elapsed() < RETRY_INTERVAL)
        {
            return;
        }
        self.last_attempt = Some(Instant::now());
        match V4l2Loopback::open() {
            Ok(camera) => {
                self.camera = Some(camera);
                self.error = None;
            }
            Err(status) => self.error = Some(status),
        }
    }
}

impl Default for LoopbackSink {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualCamera for LoopbackSink {
    fn name(&self) -> String {
        match &self.camera {
            Some(camera) => camera.name(),
            None => DEVICE_NAME.to_string(),
        }
    }

    fn status(&self) -> SinkStatus {
        match (&self.camera, &self.error) {
            (Some(camera), _) => camera.status(),
            // Recompute the hint so it follows the module loading progress.
            (None, Some(SinkStatus::NotInstalled { .. }) | None) => SinkStatus::NotInstalled {
                hint: not_installed_hint(),
            },
            (None, Some(other)) => other.clone(),
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.try_open();
        let Some(camera) = self.camera.as_mut() else {
            return Ok(());
        };
        let result = camera.send(frame);
        if result.is_err() {
            // The device went away (e.g. module unloaded); look for it again.
            self.camera = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_script_is_valid_shell() {
        let script = load_script();
        println!("{script}");
        let status = Command::new("/bin/sh")
            .args(["-n", "-c", &script])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(script.contains(&format!("card_label=\"{DEVICE_NAME}\"")));
        assert!(script.contains("exit 3"));
    }

    #[test]
    fn finds_dkms_module() {
        let root = std::env::temp_dir().join(format!("cc-modules-{}", std::process::id()));
        let dkms = root.join("updates/dkms");
        std::fs::create_dir_all(&dkms).unwrap();
        assert!(!has_loopback_module(&root));
        std::fs::write(dkms.join("v4l2loopback.ko.xz"), b"").unwrap();
        assert!(has_loopback_module(&root));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    #[ignore = "inspects the host's /lib/modules"]
    fn host_module_check() {
        println!("{:?}", check_module_for_running_kernel());
    }
}
