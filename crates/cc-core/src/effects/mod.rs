//! Pipeline steps.
//!
//! Background effects build up a *background layer* in the [`FrameContext`];
//! [`Composite`] then blends the person (from the camera frame) over it using
//! the segmentation mask. Post effects such as [`Portrait`] run on the result.
//!
//! Order matters and is fixed by [`crate::Pipeline::new`]:
//! `BackgroundImage → BlurBackground → Composite → Portrait`.
//! With both background toggles on, the selected image is blurred (once, and
//! cached) instead of the camera background.

mod background;
mod composite;
mod portrait;

pub use background::{BackgroundImage, BlurBackground, blur_sigma};
pub use composite::{Composite, composite};
pub use portrait::Portrait;

use std::sync::Arc;

/// The layer the person is composited over.
#[derive(Clone, Default)]
pub enum Background {
    /// The camera's own background, i.e. no background effect.
    #[default]
    Camera,
    /// Per-frame buffer (e.g. the blurred camera frame).
    Owned(Vec<u8>),
    /// Cached buffer reused across frames (e.g. the background image).
    Shared(Arc<Vec<u8>>),
}

impl Background {
    pub fn as_slice(&self) -> Option<&[u8]> {
        match self {
            Background::Camera => None,
            Background::Owned(v) => Some(v),
            Background::Shared(v) => Some(v),
        }
    }

    pub fn is_camera(&self) -> bool {
        matches!(self, Background::Camera)
    }
}

/// Per-frame state shared between steps.
pub struct FrameContext<'a> {
    /// Foreground mask at frame resolution, if segmentation is available.
    pub mask: Option<&'a crate::Mask>,
    pub config: &'a crate::AppConfig,
    pub background: Background,
    /// Set by [`Composite`] once the background layer was blended in.
    pub background_replaced: bool,
}
