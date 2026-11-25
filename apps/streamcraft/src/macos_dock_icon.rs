//! macOS Dock icon policy.
//!
//! Caller: `app.rs` (kept as hook). We intentionally do **not** call
//! `NSApplication.setApplicationIconImage` — that paints a sharp square and
//! cover-crops. Dock uses `Contents/Resources/AppIcon.icns` (full-bleed opaque
//! square); macOS applies the system squircle mask.
//! User: "bords arrondi et pas zoomé???? PAS RIKIKI"

#![cfg(target_os = "macos")]

/// No-op: bundle `AppIcon.icns` only. Do not override at runtime.
pub fn ensure_applied() {}
