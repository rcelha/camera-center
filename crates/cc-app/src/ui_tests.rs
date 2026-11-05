//! Headless UI rendering tests (freya-testing).
//!
//! Set `CC_UI_SNAPSHOT_DIR=<dir>` to also write PNG snapshots of each page.

use std::sync::Arc;
use std::time::Duration;

use cc_core::{AppConfig, Segmenter};
use cc_engine::{Engine, EngineDeps, FrameSource, TestPattern};
use cc_virtualcam::{NullCamera, SinkStatus, VirtualCamera};
use freya::prelude::Size2D;
use freya_core::element::AppComponent;
use freya_testing::TestingNode;
use freya_testing::prelude::*;

use crate::ui::{CameraCenterApp, Page};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 820.0;

fn runner(page: Page) -> (TestingRunner, Arc<Engine>) {
    runner_with_size(page, HEIGHT)
}

fn runner_with_size(page: Page, height: f32) -> (TestingRunner, Arc<Engine>) {
    let (preview_tx, previews) = async_channel::bounded(1);
    let (status_tx, statuses) = async_channel::bounded(4);
    let deps = EngineDeps {
        source: Box::new(|cfg: &AppConfig| {
            Ok(Box::new(TestPattern::new(cfg.resolution.max_height())) as Box<dyn FrameSource>)
        }),
        segmenter: Box::new(|_| {
            anyhow::bail!("no model in UI tests") as anyhow::Result<Box<dyn Segmenter>>
        }),
        sink: Box::new(|| {
            Box::new(NullCamera::with_status(SinkStatus::NotInstalled {
                hint: "Install the camera extension".into(),
            })) as Box<dyn VirtualCamera>
        }),
    };
    let engine = Arc::new(Engine::start(
        AppConfig::default(),
        deps,
        Box::new(move |p| {
            let _ = preview_tx.try_send(p);
        }),
    ));
    // Let the engine produce a frame and a status update.
    std::thread::sleep(Duration::from_millis(400));
    let _ = status_tx.try_send(engine.status());

    let config_path = std::env::temp_dir().join(format!("cc-ui-test-{}.toml", std::process::id()));
    let app = CameraCenterApp {
        engine: engine.clone(),
        config_path: Arc::new(config_path),
        initial_config: AppConfig::default(),
        previews,
        statuses,
        initial_page: page,
    };
    let (mut runner, _) = TestingRunner::new(
        AppComponent::new(app),
        Size2D::new(WIDTH, height),
        |_| {},
        1.0,
    );
    runner.poll(Duration::from_millis(16), Duration::from_millis(300));
    (runner, engine)
}

/// Elements whose right edge is outside the window (pushed off by a layout bug).
///
/// Absolutely positioned subtrees are skipped: they are overlays placed with
/// offsets, not part of the flow. Freya's ScrollView scrollbar is one — its
/// layout box starts at the content's right edge and it is drawn shifted back
/// by its own width, so it only shows up here once the page is tall enough to
/// scroll (e.g. with Windows' larger default font metrics).
fn overflowing(runner: &TestingRunner) -> Vec<String> {
    fn visit(node: TestingNode, out: &mut Vec<String>) {
        if node.element().layout().layout.position.is_absolute() {
            return;
        }
        let area = node.layout().area;
        if area.max_x() > WIDTH + 1.0 && area.width() > 0.0 {
            out.push(format!("{area:?}"));
        }
        for child in node.children() {
            visit(child, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = runner.find(|node, _| Some(node)) {
        visit(root, &mut out);
    }
    out
}

/// Runs the overflow check on a window short enough that the page scrolls on
/// every platform, so the scrollbar case is covered locally, not just on Windows.
#[test]
fn main_page_fits_short_window() {
    let (runner, _engine) = runner_with_size(Page::Main, 520.0);
    let overflow = overflowing(&runner);
    assert!(
        overflow.is_empty(),
        "main (short): elements overflow the window: {overflow:#?}"
    );
}

fn check_page(page: Page, name: &str) {
    let (mut runner, _engine) = runner(page);
    if let Some(dir) = std::env::var_os("CC_UI_SNAPSHOT_DIR") {
        runner.render_to_file(std::path::Path::new(&dir).join(format!("{name}.png")));
    }
    let overflow = overflowing(&runner);
    assert!(
        overflow.is_empty(),
        "{name}: elements overflow the window: {overflow:#?}"
    );
}

#[test]
fn main_page_fits_window() {
    check_page(Page::Main, "main");
}

#[test]
fn settings_page_fits_window() {
    check_page(Page::Settings, "settings");
}
