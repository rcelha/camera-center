//! Headless end-to-end test with a synthetic camera, a fake segmenter and a
//! recording virtual camera.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use cc_core::{AppConfig, EffectToggles, Frame, Mask, ProcessingResolution, Segmenter};
use cc_engine::{Engine, EngineDeps, FrameSource};
use cc_virtualcam::{SinkStatus, VirtualCamera};

struct Synthetic {
    w: u32,
    h: u32,
}

impl FrameSource for Synthetic {
    fn describe(&self) -> String {
        format!("synthetic {}x{}", self.w, self.h)
    }

    fn next_frame(&mut self) -> Result<Frame> {
        std::thread::sleep(Duration::from_millis(5));
        Ok(Frame::filled(self.w, self.h, [200, 100, 50, 255]))
    }
}

/// Marks the whole frame as background.
struct AllBackground;

impl Segmenter for AllBackground {
    fn input_size(&self) -> (u32, u32) {
        (320, 320)
    }

    fn segment(&mut self, _frame: &Frame) -> Result<Mask> {
        Ok(Mask::filled(320, 320, 0))
    }
}

struct Recorder {
    last: Arc<Mutex<Option<Frame>>>,
    count: Arc<AtomicU64>,
}

impl VirtualCamera for Recorder {
    fn name(&self) -> String {
        "recorder".into()
    }

    fn status(&self) -> SinkStatus {
        SinkStatus::Streaming
    }

    fn send(&mut self, frame: &Frame) -> Result<()> {
        *self.last.lock().unwrap() = Some(frame.clone());
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn frames_flow_through_pipeline_to_sink_and_preview() {
    let last = Arc::new(Mutex::new(None));
    let count = Arc::new(AtomicU64::new(0));
    let previews = Arc::new(AtomicU64::new(0));
    let (l, c, p) = (last.clone(), count.clone(), previews.clone());

    let deps = EngineDeps {
        source: Box::new(|cfg: &AppConfig| {
            let h = cfg.resolution.max_height().unwrap_or(1080);
            Ok(Box::new(Synthetic { w: h * 16 / 9, h }) as Box<dyn FrameSource>)
        }),
        segmenter: Box::new(|_| Ok(Box::new(AllBackground) as Box<dyn Segmenter>)),
        sink: Box::new(move || {
            Box::new(Recorder {
                last: l.clone(),
                count: c.clone(),
            }) as Box<dyn VirtualCamera>
        }),
    };
    let cfg = AppConfig {
        resolution: ProcessingResolution::P480,
        ..Default::default()
    };
    let engine = Engine::start(
        cfg.clone(),
        deps,
        Box::new(move |_| {
            p.fetch_add(1, Ordering::Relaxed);
        }),
    );

    assert!(wait_for(Duration::from_secs(5), || count
        .load(Ordering::Relaxed)
        > 3));
    assert!(previews.load(Ordering::Relaxed) > 0);
    {
        let f = last.lock().unwrap().clone().unwrap();
        assert_eq!(f.height, 480);
        assert_eq!(
            &f.data[..4],
            &[200, 100, 50, 255],
            "passthrough without effects"
        );
    }

    // Enable a background image: once the mask arrives, the whole frame
    // (all background) becomes the image colour.
    let bg = std::env::temp_dir().join(format!("cc-engine-bg-{}.png", std::process::id()));
    image_png(&bg, [10, 220, 30, 255]);
    engine.set_config(AppConfig {
        background_image: Some(bg.clone()),
        effects: EffectToggles {
            background_image: true,
            ..Default::default()
        },
        ..cfg.clone()
    });
    assert!(
        wait_for(Duration::from_secs(5), || last
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|f| f.data[..4] == [10, 220, 30, 255])),
        "background image never applied"
    );

    // Resolution change reopens the source.
    engine.set_config(AppConfig {
        resolution: ProcessingResolution::P720,
        ..cfg
    });
    assert!(wait_for(Duration::from_secs(5), || last
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|f| f.height == 720)));

    let status = engine.status();
    assert_eq!(status.sink_name, "recorder");
    assert!(status.camera.description.unwrap().contains("synthetic"));
    drop(engine);
    std::fs::remove_file(bg).ok();
}

fn image_png(path: &std::path::Path, rgba: [u8; 4]) {
    image::RgbaImage::from_pixel(8, 8, image::Rgba(rgba))
        .save(path)
        .unwrap();
}
