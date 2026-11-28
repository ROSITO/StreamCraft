//! Linux WebView host prep for wry + eframe/winit.
//!
//! wry `WebViewBuilder::build_as_child` only accepts an **X11** parent handle.
//! On Wayland (CachyOS / Arch / KDE / GNOME default) that fails with
//! `the window handle kind is not supported`.
//!
//! Fix: build the winit event loop with `with_x11()` (XWayland) unless the user
//! sets `WINIT_UNIX_BACKEND=wayland`, init GTK on X11 too, and pump the GTK loop each frame.
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
        // wry downcasts GDK's default display to X11 and panics otherwise, so GTK
        // must use the same backend as winit. GTK picks Wayland when WAYLAND_DISPLAY
        // is set; this must run before gtk::init opens the display.
        if force_x11() {
            gtk::gdk::set_allowed_backends("x11");
            // WebKit's DMA-BUF renderer fails to allocate GBM buffers on some GPUs
            // (NVIDIA under XWayland: "Failed to create GBM buffer"), leaving the
            // page blank. The user can still set the variable to 0.
            if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
                // SAFETY: called at the top of main, before any other thread exists.
                unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
            }
        }
        if let Err(e) = gtk::init() {
            tracing::error!("gtk::init failed — WebView will not work: {e}");
        } else {
            tracing::debug!("gtk::init ok");
        }
    });
}

/// winit's Xlib error handler is process-wide: X errors raised on GTK/WebKit's own
/// X connections land in winit's `latest_error`, and winit's next checked call
/// panics on them (CachyOS 0.4.1: "Failed to focus input context: GLXBadWindow").
/// Errors from any display other than winit's are marked handled, so winit only
/// reacts to its own. Call once the event loop exists (app creation).
pub fn isolate_x_errors(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    let Ok(handle) = cc.display_handle() else { return };
    let RawDisplayHandle::Xlib(xlib) = handle.as_raw() else { return };
    let Some(ours) = xlib.display else { return };
    let ours = ours.as_ptr() as usize;
    winit::platform::x11::register_xlib_error_hook(Box::new(move |display, _event| {
        let foreign = display as usize != ours;
        if foreign {
            tracing::debug!("ignored X error from a GTK/WebKit X connection");
        }
        foreign
    }));
}

/// X11 for both winit and GTK unless `WINIT_UNIX_BACKEND=wayland`.
fn force_x11() -> bool {
    !std::env::var("WINIT_UNIX_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
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
    if !force_x11() {
        tracing::info!("Linux: native Wayland requested — in-app browser embed unavailable");
        return None;
    }
    tracing::info!("Linux: forcing X11 event loop (required for wry child WebView)");
    Some(Box::new(|builder| {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }))
}
