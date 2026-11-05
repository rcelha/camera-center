use cc_core::{ProcessingResolution, SegmentationModel};
use freya::prelude::*;

use super::widgets::{card, muted, setting_row};
use crate::controller::Controller;

#[derive(PartialEq)]
pub struct SettingsPage {
    pub ctl: Controller,
}

fn load_devices() -> Result<Vec<String>, String> {
    cc_capture::list_devices()
        .map(|d| d.into_iter().map(|d| d.name).collect())
        .map_err(|e| format!("{e:#}"))
}

fn select<T: Copy + PartialEq + 'static>(
    current: T,
    options: Vec<(T, String)>,
    on_select: impl Fn(T) + Clone + 'static,
) -> Select {
    let selected = options
        .iter()
        .find(|(v, _)| *v == current)
        .map(|(_, l)| l.clone())
        .unwrap_or_default();
    Select::new()
        .selected_item(selected)
        .children(options.into_iter().map(|(value, text)| {
            let on_select = on_select.clone();
            MenuItem::new()
                .selected(value == current)
                .on_press(move |_| on_select(value))
                .child(text)
                .into()
        }))
}

impl Component for SettingsPage {
    fn render(&self) -> impl IntoElement {
        let cfg = self.ctl.config.read().clone();
        let mut devices = use_state(load_devices);

        // Input device.
        let device_names = devices.read().clone().unwrap_or_default();
        let current_device = cfg
            .input_device
            .clone()
            .filter(|d| device_names.contains(d))
            .or_else(|| device_names.first().cloned());
        let mut options: Vec<(usize, String)> = device_names.iter().cloned().enumerate().collect();
        if options.is_empty() {
            options.push((usize::MAX, "No cameras found".into()));
        }
        let current_idx = current_device
            .as_ref()
            .and_then(|d| device_names.iter().position(|n| n == d))
            .unwrap_or(usize::MAX);
        let ctl = self.ctl.clone();
        let names = device_names.clone();
        let device_select = select(current_idx, options, move |i| {
            if let Some(name) = names.get(i).cloned() {
                ctl.update(|c| c.input_device = Some(name));
            }
        });
        let refresh = Button::new()
            .flat()
            .on_press(move |_| devices.set(load_devices()))
            .child("Refresh");

        // Resolution.
        let ctl = self.ctl.clone();
        let resolution = select(
            cfg.resolution,
            ProcessingResolution::ALL
                .iter()
                .map(|r| (*r, r.label().to_string()))
                .collect(),
            move |r| ctl.update(|c| c.resolution = r),
        );

        // Model.
        let ctl = self.ctl.clone();
        let model = select(
            cfg.model,
            SegmentationModel::ALL
                .iter()
                .map(|m| (*m, m.label().to_string()))
                .collect(),
            move |m| ctl.update(|c| c.model = m),
        );

        // Background image.
        let ctl = self.ctl.clone();
        let choose = Button::new()
            .on_press(move |_| {
                let ctl = ctl.clone();
                spawn(async move {
                    let picked = rfd::AsyncFileDialog::new()
                        .set_title("Choose a background image")
                        .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
                        .pick_file()
                        .await;
                    if let Some(file) = picked {
                        let path = file.path().to_path_buf();
                        ctl.update(|c| {
                            c.background_image = Some(path);
                            c.effects.background_image = true;
                        });
                    }
                });
            })
            .child("Choose…");
        let ctl = self.ctl.clone();
        let clear = Button::new()
            .flat()
            .enabled(cfg.background_image.is_some())
            .on_press(move |_| {
                ctl.update(|c| {
                    c.background_image = None;
                    c.effects.background_image = false;
                })
            })
            .child("Clear");
        let image_path = cfg
            .background_image
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "No image selected".into());

        // Blur strength.
        let ctl = self.ctl.clone();
        let blur = Slider::new(move |v: f64| {
            let v = v.round().clamp(0.0, 100.0) as u8;
            ctl.update(|c| c.blur_strength = v);
        })
        .value(cfg.blur_strength as f64)
        .size(Size::px(260.));

        let device_error = devices
            .read()
            .as_ref()
            .err()
            .map(|e| muted(format!("Could not list cameras: {e}")));

        ScrollView::new().child(
            rect()
                .width(Size::fill())
                .padding(20.)
                .spacing(16.)
                .child(
                    card("Input")
                        .child(setting_row(
                            "Camera",
                            "The physical camera to capture from.",
                            rect()
                                .horizontal()
                                .spacing(8.)
                                .cross_align(Alignment::Center)
                                .child(device_select)
                                .child(refresh),
                        ))
                        .maybe_child(device_error)
                        .child(setting_row(
                            "Processing resolution",
                            "Lower resolutions use less CPU. The virtual camera always outputs 1920x1080.",
                            resolution,
                        )),
                )
                .child(
                    card("Background")
                        .child(setting_row(
                            "Background image",
                            &image_path,
                            rect()
                                .horizontal()
                                .spacing(8.)
                                .child(choose)
                                .child(clear),
                        ))
                        .child(setting_row(
                            "Blur strength",
                            &format!("{}%", cfg.blur_strength),
                            blur,
                        )),
                )
                .child(card("Segmentation").child(setting_row(
                    "Model",
                    "U²-Net human seg gives cleaner edges; U²-Net-p is faster on low-end machines.",
                    model,
                ))),
        )
    }
}
