//! Synthetic camera for development and tests (`CAMERA_CENTER_SOURCE=test-pattern`).

use std::time::{Duration, Instant};

use anyhow::Result;
use cc_core::Frame;

use crate::FrameSource;

/// Animated gradient background with a head-and-shoulders silhouette.
pub struct TestPattern {
    width: u32,
    height: u32,
    started: Instant,
    next: Instant,
}

impl TestPattern {
    pub fn new(max_height: Option<u32>) -> Self {
        let height = max_height.unwrap_or(1080).min(1080);
        Self {
            width: height * 16 / 9,
            height,
            started: Instant::now(),
            next: Instant::now(),
        }
    }
}

impl FrameSource for TestPattern {
    fn describe(&self) -> String {
        format!("Test pattern — {}x{} @ 30 fps", self.width, self.height)
    }

    fn next_frame(&mut self) -> Result<Frame> {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        }
        self.next = Instant::now() + Duration::from_millis(33);
        let t = self.started.elapsed().as_secs_f32();
        let (w, h) = (self.width as f32, self.height as f32);
        let sway = (t * 0.8).sin() * w * 0.05;
        let mut data = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                let (fx, fy) = (x as f32, y as f32);
                let (hx, hy) = (fx - (w * 0.5 + sway), fy - h * 0.42);
                let head = (hx / (h * 0.14)).powi(2) + (hy / (h * 0.18)).powi(2) < 1.0;
                let body =
                    fy > h * 0.62 && (fx - (w * 0.5 + sway)).abs() < h * 0.32 - (h - fy) * 0.1;
                let px = if head || body {
                    [222, 178, 150, 255]
                } else {
                    // Bookshelf-like stripes so blur is visible.
                    let stripe = ((fx / 40.0 + t).sin() * 0.5 + 0.5) * 60.0;
                    [
                        (40.0 + stripe + fy / h * 40.0) as u8,
                        (70.0 + fx / w * 80.0) as u8,
                        (120.0 - stripe * 0.5) as u8,
                        255,
                    ]
                };
                data.extend_from_slice(&px);
            }
        }
        Frame::new(self.width, self.height, data)
    }
}
