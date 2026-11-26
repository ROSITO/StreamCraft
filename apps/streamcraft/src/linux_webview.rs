//! Linux WebView host prep for wry + eframe/winit.
//!
//! wry `WebViewBuilder::build_as_child` only accepts an **X11** parent handle.
//! On Wayland (CachyOS / Arch / KDE / GNOME default) that fails with
//! `the window handle kind is not supported`.
//!
//! Fix: force winit onto X11 (XWayland) unless the user already set
//! `WINIT_UNIX_BACKEND`, init GTK, and pump the GTK loop each frame.
//!
//! Callers: `main.rs` (`prepare_host`), `app.rs` `StreamCraftApp::logic` (`pump_gtk`).
//! No prior Linux webview host module existed under `apps/streamcraft/src/`.
//! User: "browser failed : the window handle kind is not supported" (CachyOS)

#![cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]

use std::sync::Once;

/// Call once before `eframe::run_native`.
pub fn prepare_host() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("WINIT_UNIX_BACKEND").is_none() {
            // Prefer X11 so wry can embed WebKitGTK as a child of the egui window.
            // Users who need native Wayland can set WINIT_UNIX_BACKEND=wayland
            // (browser embed will still fail until wry gains Wayland child support).
            unsafe {
                std::env::set_var("WINIT_UNIX_BACKEND", "x11");
            }
            tracing::info!(
                "Linux: WINIT_UNIX_BACKEND=x11 (required for in-app browser / wry child WebView)"
            );
        } else {
            tracing::info!(
                backend = %std::env::var("WINIT_UNIX_BACKEND").unwrap_or_default(),
                "Linux: WINIT_UNIX_BACKEND already set"
            );
        }

        if let Err(e) = gtk::init() {
            tracing::error!("gtk::init failed — WebView will not work: {e}");
        } else {
            tracing::debug!("gtk::init ok");
        }
    });
}

/// Advance the GTK event loop (must run alongside eframe/winit).
pub fn pump_gtk() {
    while gtk::events_pending() {
        let _ = gtk::main_iteration_do(false);
    }
}
