use std::path::PathBuf;
use std::sync::Arc;

use cc_core::AppConfig;
use cc_engine::Engine;
use freya::prelude::*;
use tracing::warn;

/// Owns the UI copy of the configuration and pushes changes to the engine
/// and to disk.
#[derive(Clone)]
pub struct Controller {
    pub engine: Arc<Engine>,
    pub config_path: Arc<PathBuf>,
    pub config: State<AppConfig>,
}

impl PartialEq for Controller {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.engine, &other.engine) && self.config == other.config
    }
}

impl Controller {
    pub fn update(&self, f: impl FnOnce(&mut AppConfig)) {
        let mut state = self.config;
        let mut next = state.peek().clone();
        f(&mut next);
        let next = next.sanitized();
        if *state.peek() == next {
            return;
        }
        self.engine.set_config(next.clone());
        if let Err(e) = next.save(&self.config_path) {
            warn!("failed to save settings: {e:#}");
        }
        state.set(next);
    }
}
