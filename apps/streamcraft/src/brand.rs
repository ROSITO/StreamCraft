//! Brand icon assets (window + sidebar).
//!
//! Callers: `main.rs` (`window_icon` → ViewportBuilder), `app.rs` (`load_sidebar_texture`
//! in `StreamCraftApp::new`). No prior brand/icon module under `apps/streamcraft/src/`.
//! User: "tu ajoute l'icone à l'app, tu la met aussi a la palce du SC dans le menu"

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon.png");

#[cfg(not(target_os = "macos"))]
pub const WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/icon-128.png");

/// Window/taskbar icon (non-macOS). On macOS the Dock uses `AppIcon.icns` instead.
#[cfg(not(target_os = "macos"))]
pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(WINDOW_ICON_PNG)
        .unwrap_or_else(|_| egui::IconData::default())
}

pub fn load_sidebar_texture(ctx: &Context) -> TextureHandle {
    match eframe::icon_data::from_png_bytes(APP_ICON_PNG) {
        Ok(icon) => {
            let image = ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            ctx.load_texture("streamcraft_brand", image, TextureOptions::LINEAR)
        }
        Err(_) => {
            let image = ColorImage::new([32, 32], vec![crate::theme::ACCENT; 32 * 32]);
            ctx.load_texture("streamcraft_brand", image, TextureOptions::LINEAR)
        }
    }
}
