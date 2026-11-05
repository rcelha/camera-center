use anyhow::Result;
use cc_core::Frame;

use crate::{DEVICE_NAME, SinkStatus, VirtualCamera};

/// Discards frames. Used when no platform backend is available and in tests.
pub struct NullCamera {
    status: SinkStatus,
    pub frames: u64,
}

impl NullCamera {
    pub fn new() -> Self {
        Self::with_status(SinkStatus::Ready)
    }

    pub fn with_status(status: SinkStatus) -> Self {
        Self { status, frames: 0 }
    }
}

impl Default for NullCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualCamera for NullCamera {
    fn name(&self) -> String {
        DEVICE_NAME.to_string()
    }

    fn status(&self) -> SinkStatus {
        self.status.clone()
    }

    fn send(&mut self, _frame: &Frame) -> Result<()> {
        self.frames += 1;
        Ok(())
    }
}
