//! macOS Dock icon.
//!
//! Caller: `app.rs` / `StreamCraftApp::new`.  
//! - Inside `StreamCraft.app` with `Resources/AppIcon.icns` → leave Dock alone
//!   (system squircle from `CFBundleIconFile`).
//! - Bare `target/release/streamcraft-app` → set Dock from embedded `AppIcon.icns`
//!   via `NSImage` (not eframe RGBA → sharp square).
//! User: "l'icone dans le dock a disparu"

#![cfg(target_os = "macos")]

use std::sync::Once;

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

const APP_ICON_ICNS: &[u8] = include_bytes!("../assets/AppIcon.icns");

static APPLIED: Once = Once::new();

/// Apply Dock icon once. Safe to call every frame.
pub fn ensure_applied() {
    APPLIED.call_once(|| {
        if bundle_icon_present() {
            tracing::debug!("Dock: using bundle AppIcon.icns");
            return;
        }
        if let Err(e) = set_from_embedded_icns() {
            tracing::warn!("Dock icon: {e}");
        }
    });
}

fn bundle_icon_present() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // …/StreamCraft.app/Contents/MacOS/<bin> → Resources/AppIcon.icns
    exe
        .parent()
        .and_then(|macos| macos.parent())
        .map(|contents| contents.join("Resources").join("AppIcon.icns"))
        .is_some_and(|p| p.is_file())
}

fn set_from_embedded_icns() -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Dock icon needs main thread")?;
    // SAFETY: NSData copies bytes; NSImage owns the representation.
    let data = NSData::with_bytes(APP_ICON_ICNS);
    let img = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or_else(|| "NSImage::initWithData failed for AppIcon.icns".to_string())?;
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: NSApp is ready on the main thread; image retained by AppKit.
    unsafe {
        app.setApplicationIconImage(Some(&img));
    }
    tracing::info!("Dock: applied embedded AppIcon.icns (bare binary)");
    Ok(())
}
