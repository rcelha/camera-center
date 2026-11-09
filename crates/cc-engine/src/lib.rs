//! Runtime engine.
//!
//! ```text
//! capture thread ──Latest<Frame>──▶ processing thread ──▶ virtual camera
//!                                    │      ▲                 └─▶ preview callback (UI)
//!                       Latest<Frame>│      │ArcSwap<Mask>
//!                                    ▼      │
//!                              segmentation thread
//! ```
//!
//! Each hand-off keeps only the newest item, so a slow stage drops frames
//! rather than adding latency. Segmentation runs asynchronously: the
//! compositor always uses the most recent mask.

mod slot;
mod test_pattern;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use arc_swap::{ArcSwap, ArcSwapOption};
use cc_core::{AppConfig, Frame, Mask, Pipeline, SegmentationModel, Segmenter};
use cc_virtualcam::{SinkStatus, VirtualCamera};
use tracing::{error, info, warn};

pub use slot::Latest;
pub use test_pattern::TestPattern;

/// Width of preview thumbnails sent to the UI.
pub const PREVIEW_WIDTH: u32 = 640;

/// Something that produces frames (a camera, or a synthetic source in tests).
///
/// Sources are created and used on the capture thread only, so they don't
/// need to be `Send` (AVFoundation-backed cameras aren't).
pub trait FrameSource {
    fn describe(&self) -> String;
    fn next_frame(&mut self) -> Result<Frame>;
}

impl FrameSource for cc_capture::CaptureSession {
    fn describe(&self) -> String {
        format!("{} — {}", self.device().name, self.format())
    }

    fn next_frame(&mut self) -> Result<Frame> {
        cc_capture::CaptureSession::next_frame(self)
    }
}

type SourceFactory = Box<dyn Fn(&AppConfig) -> Result<Box<dyn FrameSource>> + Send + Sync>;
type SegmenterFactory = Box<dyn Fn(SegmentationModel) -> Result<Box<dyn Segmenter>> + Send + Sync>;
type SinkFactory = Box<dyn Fn() -> Box<dyn VirtualCamera> + Send>;
type PreviewCallback = Box<dyn Fn(Previews) + Send>;

/// Pluggable dependencies of the engine.
pub struct EngineDeps {
    pub source: SourceFactory,
    pub segmenter: SegmenterFactory,
    pub sink: SinkFactory,
}

impl EngineDeps {
    /// Real camera, ONNX segmentation and the platform virtual camera.
    pub fn platform() -> Self {
        Self {
            source: Box::new(|cfg: &AppConfig| {
                let session = cc_capture::CaptureSession::open(
                    cfg.input_device.as_deref(),
                    cfg.resolution.max_height(),
                )?;
                Ok(Box::new(session) as Box<dyn FrameSource>)
            }),
            segmenter: Box::new(|model| {
                let path = cc_ml::model_path(model)?;
                let seg = cc_ml::U2Net::load(&path, cc_ml::ExecutionBackend::Auto)?;
                Ok(Box::new(seg) as Box<dyn Segmenter>)
            }),
            sink: Box::new(cc_virtualcam::open_platform_camera),
        }
    }
}

/// Original and processed thumbnails for the UI.
#[derive(Clone)]
pub struct Previews {
    pub original: Frame,
    pub processed: Frame,
}

#[derive(Debug, Clone, Default)]
pub struct CameraStatus {
    pub description: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SegmentationStatus {
    pub model: Option<SegmentationModel>,
    pub loading: bool,
    pub error: Option<String>,
    pub last_ms: f32,
}

#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub camera: CameraStatus,
    pub segmentation: SegmentationStatus,
    pub sink_name: String,
    pub sink_status: SinkStatus,
    pub output_fps: f32,
    pub process_ms: f32,
    pub processing_resolution: Option<(u32, u32)>,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            camera: CameraStatus::default(),
            segmentation: SegmentationStatus::default(),
            sink_name: cc_virtualcam::DEVICE_NAME.to_string(),
            sink_status: SinkStatus::Ready,
            output_fps: 0.0,
            process_ms: 0.0,
            processing_resolution: None,
        }
    }
}

struct Shared {
    config: ArcSwap<AppConfig>,
    status: Mutex<EngineStatus>,
    frames: Latest<Frame>,
    seg_input: Latest<Frame>,
    mask: ArcSwapOption<Mask>,
    stop: AtomicBool,
}

impl Shared {
    fn status(&self, f: impl FnOnce(&mut EngineStatus)) {
        f(&mut self.status.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Handle to the running engine. Dropping it stops all threads.
pub struct Engine {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Engine {
    pub fn start(config: AppConfig, deps: EngineDeps, on_preview: PreviewCallback) -> Engine {
        let shared = Arc::new(Shared {
            config: ArcSwap::from_pointee(config),
            status: Mutex::new(EngineStatus::default()),
            frames: Latest::default(),
            seg_input: Latest::default(),
            mask: ArcSwapOption::empty(),
            stop: AtomicBool::new(false),
        });
        let EngineDeps {
            source,
            segmenter,
            sink,
        } = deps;
        let threads = vec![
            spawn("cc-capture", shared.clone(), move |s| {
                capture_loop(&s, &source)
            }),
            spawn("cc-segment", shared.clone(), move |s| {
                segmentation_loop(&s, &segmenter)
            }),
            spawn("cc-process", shared.clone(), move |s| {
                processing_loop(&s, sink, on_preview)
            }),
        ];
        Engine { shared, threads }
    }

    pub fn config(&self) -> Arc<AppConfig> {
        self.shared.config.load_full()
    }

    /// Applies a new configuration; capture and model changes take effect
    /// on the next frame.
    pub fn set_config(&self, config: AppConfig) {
        self.shared.config.store(Arc::new(config));
    }

    pub fn status(&self) -> EngineStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.frames.wake();
        self.shared.seg_input.wake();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn spawn(
    name: &str,
    shared: Arc<Shared>,
    f: impl FnOnce(Arc<Shared>) + Send + 'static,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || f(shared))
        .expect("failed to spawn engine thread")
}

/// Settings that require reopening the camera.
fn capture_key(cfg: &AppConfig) -> (Option<String>, Option<u32>) {
    (cfg.input_device.clone(), cfg.resolution.max_height())
}

fn capture_loop(shared: &Shared, factory: &SourceFactory) {
    let mut source: Option<(Box<dyn FrameSource>, _)> = None;
    let mut seq = 0u64;
    while !shared.stopped() {
        let cfg = shared.config.load_full();
        let key = capture_key(&cfg);
        if source.as_ref().is_some_and(|(_, k)| *k != key) {
            source = None; // settings changed: reopen
        }
        let (src, _) = match &mut source {
            Some(s) => s,
            None => match factory(&cfg) {
                Ok(src) => {
                    let description = src.describe();
                    info!(%description, "capture started");
                    shared.status(|s| {
                        s.camera = CameraStatus {
                            description: Some(description),
                            error: None,
                        }
                    });
                    source.insert((src, key))
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    warn!("camera unavailable: {msg}");
                    shared.status(|s| s.camera.error = Some(msg));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            },
        };
        match src
            .next_frame()
            .and_then(|f| f.fit_height(cfg.resolution.max_height()))
        {
            Ok(frame) => {
                seq += 1;
                shared.frames.put(frame.with_seq(seq));
            }
            Err(e) => {
                let msg = format!("{e:#}");
                error!("capture failed: {msg}");
                shared.status(|s| s.camera.error = Some(msg));
                source = None;
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn segmentation_loop(shared: &Shared, factory: &SegmenterFactory) {
    let mut current: Option<(SegmentationModel, Box<dyn Segmenter>)> = None;
    let mut failed: Option<SegmentationModel> = None;
    let mut smoother = cc_ml::TemporalSmoother::new(0.6);
    while !shared.stopped() {
        let Some(frame) = shared.seg_input.take(Duration::from_millis(200)) else {
            continue;
        };
        let model = shared.config.load().model;
        if current.as_ref().is_none_or(|(m, _)| *m != model) {
            if failed == Some(model) {
                continue;
            }
            current = None;
            smoother.reset();
            shared.mask.store(None);
            shared.status(|s| {
                s.segmentation = SegmentationStatus {
                    model: Some(model),
                    loading: true,
                    ..Default::default()
                }
            });
            match factory(model) {
                Ok(seg) => {
                    current = Some((model, seg));
                    failed = None;
                    shared.status(|s| s.segmentation.loading = false);
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    error!("failed to load {model:?}: {msg}");
                    failed = Some(model);
                    shared.status(|s| {
                        s.segmentation.loading = false;
                        s.segmentation.error = Some(msg);
                    });
                    continue;
                }
            }
        }
        let Some((_, seg)) = current.as_mut() else {
            continue;
        };
        let started = Instant::now();
        match seg.segment(&frame) {
            Ok(mask) => {
                shared.mask.store(Some(Arc::new(smoother.apply(mask))));
                let ms = started.elapsed().as_secs_f32() * 1000.0;
                shared.status(|s| {
                    s.segmentation.last_ms = ms;
                    s.segmentation.error = None;
                });
            }
            Err(e) => {
                let msg = format!("{e:#}");
                warn!("segmentation failed: {msg}");
                shared.status(|s| s.segmentation.error = Some(msg));
            }
        }
    }
}

fn processing_loop(shared: &Shared, sink: SinkFactory, on_preview: PreviewCallback) {
    let mut sink = sink();
    let sink_name = sink.name();
    shared.status(|s| {
        s.sink_name = sink_name;
        s.sink_status = sink.status();
    });
    let mut pipeline = Pipeline::new();
    let mut fps = FpsCounter::default();
    let mut sink_error_logged = false;

    while !shared.stopped() {
        let Some(frame) = shared.frames.take(Duration::from_millis(200)) else {
            shared.status(|s| s.sink_status = sink.status());
            continue;
        };
        let started = Instant::now();
        let cfg = shared.config.load_full();

        let mask = if pipeline.needs_mask(&cfg) {
            let (iw, ih) = (320, 320);
            if let Ok(small) = frame.resized(iw, ih) {
                shared.seg_input.put(small);
            }
            shared.mask.load_full()
        } else {
            None
        };

        let original = frame.thumbnail(PREVIEW_WIDTH, PREVIEW_WIDTH);
        let mut processed = frame;
        if let Err(e) = pipeline.process(&mut processed, mask.as_ref(), &cfg) {
            warn!("pipeline failed: {e:#}");
        }

        match sink.send(&processed) {
            Ok(()) => sink_error_logged = false,
            Err(e) => {
                if !sink_error_logged {
                    warn!("virtual camera: {e:#}");
                    sink_error_logged = true;
                }
            }
        }

        let process_ms = started.elapsed().as_secs_f32() * 1000.0;
        let out_fps = fps.tick();
        let dims = (processed.width, processed.height);
        shared.status(|s| {
            s.process_ms = process_ms;
            s.output_fps = out_fps;
            s.processing_resolution = Some(dims);
            s.sink_name = sink.name();
            s.sink_status = sink.status();
        });

        if let (Ok(original), Ok(processed)) =
            (original, processed.thumbnail(PREVIEW_WIDTH, PREVIEW_WIDTH))
        {
            on_preview(Previews {
                original,
                processed,
            });
        }
    }
}

#[derive(Default)]
struct FpsCounter {
    window_start: Option<Instant>,
    frames: u32,
    fps: f32,
}

impl FpsCounter {
    fn tick(&mut self) -> f32 {
        let now = Instant::now();
        let start = *self.window_start.get_or_insert(now);
        self.frames += 1;
        let elapsed = now.duration_since(start).as_secs_f32();
        if elapsed >= 1.0 {
            self.fps = self.frames as f32 / elapsed;
            self.frames = 0;
            self.window_start = Some(now);
        }
        self.fps
    }
}
