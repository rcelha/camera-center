mod main_page;
mod settings_page;
mod widgets;

use std::path::PathBuf;
use std::sync::Arc;

use cc_core::{AppConfig, Frame};
use cc_engine::{Engine, EngineStatus, Previews};
use freya::elements::image::ImageHandle;
use freya::engine::prelude::AlphaType;
use freya::prelude::*;

use crate::controller::Controller;

pub struct CameraCenterApp {
    pub engine: Arc<Engine>,
    pub config_path: Arc<PathBuf>,
    pub initial_config: AppConfig,
    pub previews: async_channel::Receiver<Previews>,
    pub statuses: async_channel::Receiver<EngineStatus>,
    pub initial_page: Page,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Main,
    Settings,
}

fn to_handle(frame: Frame) -> Option<ImageHandle> {
    ImageHandle::from_rgba(
        frame.width,
        frame.height,
        Bytes::from(frame.data),
        AlphaType::Unpremul,
    )
}

impl App for CameraCenterApp {
    fn render(&self) -> impl IntoElement {
        use_init_theme(dark_theme);
        let initial_page = self.initial_page;
        let page = use_state(move || initial_page);
        let config = use_state(|| self.initial_config.clone());
        let mut original = use_state(|| None::<ImageHandle>);
        let mut processed = use_state(|| None::<ImageHandle>);
        let mut status = use_state(EngineStatus::default);

        let previews = self.previews.clone();
        let statuses = self.statuses.clone();
        use_hook(move || {
            spawn(async move {
                while let Ok(p) = previews.recv().await {
                    original.set(to_handle(p.original));
                    processed.set(to_handle(p.processed));
                }
            });
            spawn(async move {
                while let Ok(s) = statuses.recv().await {
                    status.set(s);
                }
            });
        });

        let ctl = Controller {
            engine: self.engine.clone(),
            config_path: self.config_path.clone(),
            config,
        };

        rect()
            .expanded()
            .background((18, 18, 22))
            .color((235, 235, 240))
            .child(header(page))
            .child(match page() {
                Page::Main => main_page::MainPage {
                    ctl,
                    original,
                    processed,
                    status,
                }
                .into_element(),
                Page::Settings => settings_page::SettingsPage { ctl }.into_element(),
            })
    }
}

fn header(page: State<Page>) -> impl IntoElement {
    rect()
        .horizontal()
        .content(Content::flex())
        .width(Size::fill())
        .padding((14., 20.))
        .spacing(10.)
        .cross_align(Alignment::Center)
        .background((28, 28, 34))
        .child(
            label()
                .text("Camera Center")
                .font_size(20.)
                .font_weight(FontWeight::BOLD),
        )
        .child(rect().width(Size::flex(1.)))
        .child(tab(page, Page::Main, "Camera"))
        .child(tab(page, Page::Settings, "Settings"))
}

fn tab(mut page: State<Page>, target: Page, text: &'static str) -> Button {
    let button = Button::new()
        .on_press(move |_| page.set(target))
        .child(text);
    if page() == target {
        button.filled()
    } else {
        button.flat()
    }
}
