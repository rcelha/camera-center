//! System tray / menu bar icon. The app keeps running (and the virtual camera
//! keeps streaming) after the main window is closed; the tray reopens it or
//! quits the app.

use freya::prelude::*;
use freya::tray::menu::{Menu, MenuItem, PredefinedMenuItem};
use freya::tray::{
    MouseButton, MouseButtonState, TrayEvent, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use tracing::{error, info};

use crate::{AppShared, main_window};

const ICON: &[u8] = include_bytes!("../assets/tray.png");
const OPEN: &str = "open";
const QUIT: &str = "quit";

/// Creates the tray icon. Freya calls this on the UI thread (a GTK thread on Linux).
pub fn build() -> TrayIcon {
    let menu = Menu::new();
    let items = [
        &MenuItem::with_id(OPEN, "Open Camera Center", true, None)
            as &dyn freya::tray::menu::IsMenuItem,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(QUIT, "Quit Camera Center", true, None),
    ];
    for item in items {
        if let Err(e) = menu.append(item) {
            error!("failed to build tray menu: {e}");
        }
    }
    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Camera Center — virtual camera running")
        .with_icon(LaunchConfig::tray_icon(ICON))
        // Monochrome template image so macOS tints it for light/dark menu bars.
        .with_icon_as_template(true)
        // macOS convention: clicking the menu bar icon opens the menu. On
        // Windows/Linux a left click opens the window and right click the menu.
        .with_menu_on_left_click(cfg!(target_os = "macos"))
        .build()
        .expect("failed to create the tray icon")
}

/// Handles tray clicks and menu selections.
pub fn handler(shared: AppShared) -> impl FnMut(TrayEvent, RendererContext) + 'static {
    move |event, mut ctx| match event {
        TrayEvent::Menu(e) if e.id == OPEN => open_window(&shared, &mut ctx),
        TrayEvent::Menu(e) if e.id == QUIT => {
            info!("quit requested from the tray");
            ctx.exit();
        }
        TrayEvent::Icon(TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }) if !cfg!(target_os = "macos") => open_window(&shared, &mut ctx),
        _ => {}
    }
}

/// Focuses the main window, or recreates it if it was closed.
fn open_window(shared: &AppShared, ctx: &mut RendererContext) {
    if let Some(window) = ctx.windows().values().next() {
        let window = window.window();
        window.set_visible(true);
        window.set_minimized(false);
        window.focus_window();
    } else {
        ctx.launch_window(main_window(shared));
    }
}
