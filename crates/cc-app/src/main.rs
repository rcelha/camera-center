//! Camera Center desktop app.

mod controller;
mod ui;
#[cfg(test)]
mod ui_tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use cc_core::AppConfig;
use cc_engine::{Engine, EngineDeps};
use freya::prelude::*;
use tracing::warn;

fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CAMERA_CENTER_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    let dirs = directories::ProjectDirs::from("com", "cameracenter", "Camera Center")
        .context("cannot determine the user config directory")?;
    Ok(dirs.config_dir().join("config.toml"))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,cc_engine=info,cc_ml=info,cc_capture=info,cc_virtualcam=info,camera_center=info".into()),
        )
        .init();

    let config_path = config_path()?;
    let config = AppConfig::load(&config_path).unwrap_or_else(|e| {
        warn!("ignoring unreadable config: {e:#}");
        AppConfig::default()
    });

    let (preview_tx, preview_rx) = async_channel::bounded(1);
    let mut deps = EngineDeps::platform();
    let open_camera = deps.source;
    let test_pattern = std::env::var("CAMERA_CENTER_SOURCE").as_deref() == Ok("test-pattern");
    deps.source = Box::new(move |cfg| {
        if test_pattern {
            return Ok(
                Box::new(cc_engine::TestPattern::new(cfg.resolution.max_height()))
                    as Box<dyn cc_engine::FrameSource>,
            );
        }
        // macOS shows the permission prompt once; this blocks only the capture thread.
        if !cc_capture::request_permission(Duration::from_secs(120)) {
            anyhow::bail!(
                "camera access denied — allow it in System Settings → Privacy & Security → Camera"
            );
        }
        open_camera(cfg)
    });
    let engine = Arc::new(Engine::start(
        config.clone(),
        deps,
        Box::new(move |previews| {
            // Drop previews if the UI is behind.
            let _ = preview_tx.try_send(previews);
        }),
    ));

    let (status_tx, status_rx) = async_channel::bounded(1);
    let weak = Arc::downgrade(&engine);
    std::thread::Builder::new()
        .name("cc-status".into())
        .spawn(move || {
            while let Some(engine) = weak.upgrade() {
                if let Err(async_channel::TrySendError::Closed(_)) =
                    status_tx.try_send(engine.status())
                {
                    break;
                }
                drop(engine);
                std::thread::sleep(Duration::from_millis(500));
            }
        })?;

    launch(
        LaunchConfig::new().with_window(
            WindowConfig::new_app(ui::CameraCenterApp {
                engine,
                config_path: Arc::new(config_path),
                initial_config: config,
                previews: preview_rx,
                statuses: status_rx,
                initial_page: ui::Page::Main,
            })
            .with_title("Camera Center")
            .with_size(1180.0, 820.0)
            .with_min_size(900.0, 640.0),
        ),
    );
    Ok(())
}
