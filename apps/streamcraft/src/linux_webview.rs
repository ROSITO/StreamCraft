//! Linux WebView host prep for wry + eframe/winit.
//!
//! wry `WebViewBuilder::build_as_child` only accepts an **X11** parent handle.
//! On Wayland (CachyOS / Arch / KDE / GNOME default) that fails with
//! `the window handle kind is not supported`.
//!
//! Fix: build the winit event loop with `with_x11()` (XWayland) unless the user
//! sets `WINIT_UNIX_BACKEND=wayland`, init GTK, and pump the GTK loop each frame.
//! winit ≥ 0.29 no longer reads `WINIT_UNIX_BACKEND` itself: it picks Wayland
//! whenever `WAYLAND_DISPLAY` is set, so an env var alone does not force X11.
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

/// eframe hook forcing the X11 backend so wry can embed WebKitGTK as a child.
/// `WINIT_UNIX_BACKEND=wayland` opts out (the in-app browser then won't embed).
pub fn x11_event_loop_hook() -> Option<eframe::EventLoopBuilderHook> {
    if std::env::var("WINIT_UNIX_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("wayland")) {
        tracing::info!("Linux: native Wayland requested — in-app browser embed unavailable");
        return None;
    }
    tracing::info!("Linux: forcing X11 event loop (required for wry child WebView)");
    Some(Box::new(|builder| {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }))
}
