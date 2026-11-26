//! StreamCraft Craft desktop host (egui). Binary: `streamcraft-app` (D021).
//! On Linux, calls `linux_webview::prepare_host` before eframe (Wayland → X11 for wry).

mod app;
mod brand;
mod browser;
mod detect;
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod linux_webview;
#[cfg(target_os = "macos")]
mod macos_dock_icon;
mod panels;
mod roles;
mod sidebar;
mod theme;

use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("streamcraft_app=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    linux_webview::prepare_host();

    // macOS: IconData::default() tells eframe NOT to call setApplicationIconImage
    // (and NOT to fall back to the egui logo). Dock then uses AppIcon.icns from
    // the .app bundle with the system squircle. A real PNG via with_icon paints
    // a sharp square tile and overrides the bundle icon.
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([960.0, 640.0])
        .with_title("StreamCraft")
        .with_active(true)
        .with_icon({
            #[cfg(target_os = "macos")]
            {
                egui::IconData::default()
            }
            #[cfg(not(target_os = "macos"))]
            {
                brand::window_icon()
            }
        });

    let options = eframe::NativeOptions {
        viewport,
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "StreamCraft",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(app::StreamCraftApp::new(cc)))
        }),
    )
}
