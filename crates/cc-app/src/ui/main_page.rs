use cc_engine::EngineStatus;
use cc_virtualcam::SinkStatus;
use freya::elements::image::ImageHandle;
use freya::prelude::*;

use super::widgets::{badge, card, muted, setting_row};
use crate::controller::Controller;

#[derive(PartialEq)]
pub struct MainPage {
    pub ctl: Controller,
    pub original: State<Option<ImageHandle>>,
    pub processed: State<Option<ImageHandle>>,
    pub status: State<EngineStatus>,
}

fn preview(title: &str, frame: Option<ImageHandle>) -> Rect {
    let body = match frame {
        Some(handle) => image(handle)
            .width(Size::fill())
            .height(Size::fill())
            .aspect_ratio(AspectRatio::Min)
            .sampling_mode(SamplingMode::Bilinear)
            .into_element(),
        None => rect()
            .expanded()
            .center()
            .child(muted("Waiting for camera…"))
            .into_element(),
    };
    rect()
        .width(Size::flex(1.))
        .spacing(8.)
        .child(
            label()
                .text(title.to_string())
                .font_size(14.)
                .font_weight(FontWeight::BOLD),
        )
        .child(
            rect()
                .width(Size::fill())
                .height(Size::px(320.))
                .corner_radius(8.)
                .background((8, 8, 10))
                .center()
                .child(body),
        )
}

fn sink_badge(status: &SinkStatus) -> Rect {
    let color = match status {
        SinkStatus::Streaming => (110, 220, 140),
        SinkStatus::Ready => (120, 180, 255),
        SinkStatus::NotInstalled { .. } => (240, 200, 90),
        SinkStatus::Error(_) => (245, 110, 110),
    };
    badge(status.label(), color)
}

fn virtual_camera_card(status: &EngineStatus) -> Rect {
    let detail = match &status.sink_status {
        SinkStatus::NotInstalled { hint } => Some(hint.clone()),
        SinkStatus::Error(e) => Some(e.clone()),
        SinkStatus::Ready => Some("Waiting for the first frame".into()),
        SinkStatus::Streaming => None,
    };
    let mut row = rect()
        .horizontal()
        .content(Content::flex())
        .width(Size::fill())
        .spacing(12.)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .width(Size::flex(1.))
                .spacing(4.)
                .child(muted("Device name"))
                .child(
                    label()
                        .text(status.sink_name.clone())
                        .font_size(18.)
                        .font_weight(FontWeight::BOLD),
                ),
        )
        .child(sink_badge(&status.sink_status));
    if can_install()
        && matches!(
            status.sink_status,
            SinkStatus::NotInstalled { .. } | SinkStatus::Error(_)
        )
    {
        row = row.child(install_button());
    }
    card("Virtual camera")
        .child(row)
        .maybe_child(detail.map(muted))
}

/// The install button is only useful from the bundled app in /Applications;
/// elsewhere the status hint explains how to get there.
#[cfg(target_os = "macos")]
fn can_install() -> bool {
    cc_virtualcam::macos::install_blocker().is_none()
}

#[cfg(not(target_os = "macos"))]
fn can_install() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn install_button() -> Button {
    Button::new()
        .on_press(|_| cc_virtualcam::macos::activate())
        .child("Install extension")
}

#[cfg(not(target_os = "macos"))]
fn install_button() -> Rect {
    rect()
}

fn stats_line(status: &EngineStatus) -> String {
    let mut parts = Vec::new();
    if let Some((w, h)) = status.processing_resolution {
        parts.push(format!("{w}x{h}"));
    }
    parts.push(format!("{:.0} fps", status.output_fps));
    parts.push(format!("pipeline {:.1} ms", status.process_ms));
    let seg = &status.segmentation;
    if seg.loading {
        parts.push("loading model…".into());
    } else if seg.last_ms > 0.0 {
        parts.push(format!("segmentation {:.0} ms", seg.last_ms));
    }
    parts.join("  ·  ")
}

impl Component for MainPage {
    fn render(&self) -> impl IntoElement {
        let status = self.status.read().clone();
        let cfg = self.ctl.config.read().clone();
        let has_image = cfg.background_image.is_some();

        let ctl = self.ctl.clone();
        let blur = Switch::new()
            .toggled(cfg.effects.blur_background)
            .on_toggle(move |_| ctl.update(|c| c.effects.blur_background ^= true));
        let ctl = self.ctl.clone();
        let image_toggle = Switch::new()
            .toggled(cfg.effects.background_image)
            .enabled(has_image)
            .on_toggle(move |_| ctl.update(|c| c.effects.background_image ^= true));
        let ctl = self.ctl.clone();
        let portrait = Switch::new()
            .toggled(cfg.effects.portrait)
            .on_toggle(move |_| ctl.update(|c| c.effects.portrait ^= true));

        let camera_line = match (&status.camera.error, &status.camera.description) {
            (Some(e), _) => format!("Camera error: {e}"),
            (None, Some(d)) => d.clone(),
            (None, None) => "Opening camera…".into(),
        };
        let seg_error = status
            .segmentation
            .error
            .clone()
            .map(|e| muted(format!("Segmentation: {e}")).color((245, 120, 120)));

        ScrollView::new().child(
            rect()
                .width(Size::fill())
                .padding(20.)
                .spacing(16.)
                .child(
                    rect()
                        .horizontal()
                        .content(Content::flex())
                        .width(Size::fill())
                        .spacing(16.)
                        .child(preview("Original", self.original.read().clone()))
                        .child(preview("Processed", self.processed.read().clone())),
                )
                .child(muted(format!("{camera_line}  ·  {}", stats_line(&status))))
                .child(virtual_camera_card(&status))
                .child(
                    card("Effects")
                        .child(setting_row(
                            "Blur background",
                            "Blurs everything behind you. With a background image on, the image is blurred instead.",
                            blur,
                        ))
                        .child(setting_row(
                            "Background image",
                            if has_image {
                                "Replaces your background with the image chosen in Settings."
                            } else {
                                "Choose an image in Settings to enable."
                            },
                            image_toggle,
                        ))
                        .child(setting_row(
                            "Portrait mode",
                            "Shallow depth of field, exposure and tone tuned for video calls.",
                            portrait,
                        ))
                        .maybe_child(seg_error),
                ),
        )
    }
}
