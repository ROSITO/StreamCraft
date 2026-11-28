//! In-app browser: wry WebView as child of the eframe window (D008 / D021).
//! Parité Swift/Tauri : `WKScriptMessageHandler` `streamcraftMedia` all-frames
//! (`macos_media_hook`) + PopupGuard JS. wry `window.ipc` reste un fallback top-frame.

use std::sync::mpsc::Sender;
#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex};

use raw_window_handle::HasWindowHandle;
use streamcraft_core::agent::popup_guard::PopupGuard;
use streamcraft_core::session::{CookieEntry, SessionCredentialSnapshot};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebView, WebViewBuilder};

#[cfg(target_os = "macos")]
mod macos_media_hook;
#[cfg(target_os = "macos")]
use macos_media_hook::PendingMedia;

/// Top-frame only helper: queue.push → ipc. Iframe delivery is handled by MEDIA_HOOKS
/// (`postMessage` → top → ipc). Keep this as a belt-and-suspenders for main-frame hits.
const IPC_BRIDGE_JS: &str = r#"(function () {
  if (window.__streamcraftIpcBridge) return;
  window.__streamcraftIpcBridge = true;
  var isTop = false;
  try { isTop = (window === window.top); } catch (e) { isTop = false; }
  if (!isTop) return;
  function emit(p) {
    try {
      window.ipc.postMessage(JSON.stringify({
        type: 'media',
        url: String((p && p.url) || ''),
        mime: (p && p.mime) || null,
        source: (p && p.source) || 'hook',
        page: (p && p.page) || (location && location.href) || null
      }));
    } catch (e) {}
  }
  function flush() {
    try {
      var q = window.__streamcraftMediaQueue || [];
      if (!q.length) return;
      window.__streamcraftMediaQueue = [];
      for (var i = 0; i < q.length; i++) emit(q[i] || {});
    } catch (e2) {}
  }
  window.__streamcraftMediaQueue = window.__streamcraftMediaQueue || [];
  var _push = Array.prototype.push;
  window.__streamcraftMediaQueue.push = function () {
    for (var i = 0; i < arguments.length; i++) emit(arguments[i]);
    return _push.apply(this, arguments);
  };
  setInterval(flush, 500);
  flush();
})();"#;

const RESCAN_JS: &str = r#"(function () {
  try {
    window.__streamcraftMediaQueue = window.__streamcraftMediaQueue || [];
    var entries = performance.getEntriesByType('resource');
    for (var i = 0; i < entries.length; i++) {
      window.__streamcraftMediaQueue.push({
        url: entries[i].name, mime: null, source: 'rescan', page: location.href
      });
    }
    document.querySelectorAll('video,audio,source').forEach(function (el) {
      var u = el.currentSrc || el.src;
      if (u) {
        window.__streamcraftMediaQueue.push({
          url: u, mime: el.getAttribute('type'), source: 'rescan', page: location.href
        });
      }
    });
    var iframes = document.querySelectorAll('iframe');
    for (var j = 0; j < iframes.length; j++) {
      var f = iframes[j];
      var src = f.src || f.getAttribute('src') || f.getAttribute('data-src');
      if (src) {
        window.__streamcraftMediaQueue.push({
          url: src, mime: null, source: 'iframeSrc', page: location.href
        });
      }
      try {
        if (f.contentWindow) {
          f.contentWindow.postMessage({ __streamcraftFlush: true }, '*');
        }
      } catch (eFlush) {}
      try {
        var doc = f.contentDocument;
        if (!doc) continue;
        doc.querySelectorAll('video,audio,source').forEach(function (el) {
          var u = el.currentSrc || el.src;
          if (u) {
            window.__streamcraftMediaQueue.push({
              url: u, mime: el.getAttribute('type'), source: 'iframeRescan', page: location.href
            });
          }
        });
      } catch (cross) {}
    }
  } catch (e) {}
  return JSON.stringify({queued: (window.__streamcraftMediaQueue || []).length});
})();"#;

const START_HTML: &str = r###"<!doctype html><html><head><meta charset="utf-8"><style>
html,body{margin:0;height:100%;background:#0e0f12;color:#f2f0eb;font:15px/1.45 -apple-system,BlinkMacSystemFont,sans-serif;display:flex;align-items:center;justify-content:center;text-align:center}
main{max-width:28rem;padding:2rem}h1{font-size:1.35rem;font-weight:600;margin:0 0 .6rem;color:#9b6bff}
p{margin:0;color:#8b8798}
</style></head><body><main>
<h1>StreamCraft Browser</h1>
<p>Paste a watch page in the bar above and press <b>Go</b>. Detected streams show up on the right.</p>
</main></body></html>"###;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BrowserMessage {
    Title(String),
    PageUrl(String),
    Candidate {
        url: String,
        mime: Option<String>,
        source: Option<String>,
        page: Option<String>,
    },
    Error(String),
}

pub struct EmbeddedBrowser {
    webview: WebView,
    #[cfg(target_os = "macos")]
    pending_media: PendingMedia,
}

impl EmbeddedBrowser {
    pub fn create(
        parent: &impl HasWindowHandle,
        tx: Sender<BrowserMessage>,
        bounds: egui::Rect,
        block_redirects: bool,
    ) -> anyhow::Result<Self> {
        let tx_ipc = tx.clone();
        let tx_nav = tx.clone();
        let tx_load = tx.clone();
        let tx_title = tx.clone();
        #[cfg(target_os = "macos")]
        let pending_media: PendingMedia = Arc::new(Mutex::new(Vec::new()));
        let builder = WebViewBuilder::new()
            .with_html(START_HTML)
            // all frames — video players often live in iframes (Swift forMainFrameOnly: false)
            .with_initialization_script_for_main_only(PopupGuard::MEDIA_HOOKS_JS, false)
            .with_initialization_script_for_main_only(PopupGuard::NEUTRALIZE_WINDOW_OPEN_JS, false)
            .with_initialization_script_for_main_only(IPC_BRIDGE_JS, false)
            // Parité Tauri `on_document_title_changed` → PageTitleResolver
            .with_document_title_changed_handler(move |title| {
                let t = title.trim();
                if !t.is_empty() {
                    let _ = tx_title.send(BrowserMessage::Title(t.to_string()));
                }
            })
            .with_ipc_handler(move |req| {
                let body = req.body();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                    if v.get("type").and_then(|t| t.as_str()) == Some("media") {
                        let url = v
                            .get("url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !url.is_empty() {
                            let mime = v
                                .get("mime")
                                .and_then(|m| m.as_str())
                                .map(str::to_string);
                            let source = v
                                .get("source")
                                .and_then(|s| s.as_str())
                                .map(str::to_string);
                            let page = v
                                .get("page")
                                .and_then(|p| p.as_str())
                                .map(str::to_string);
                            tracing::debug!(%url, "browser ipc media");
                            let _ = tx_ipc.send(BrowserMessage::Candidate {
                                url,
                                mime,
                                source,
                                page,
                            });
                        }
                    }
                } else if body.starts_with('[') {
                    // drain callback JSON array
                    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(body) {
                        for v in arr {
                            let url = v
                                .get("url")
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                            if url.is_empty() {
                                continue;
                            }
                            let mime = v
                                .get("mime")
                                .and_then(|m| m.as_str())
                                .map(str::to_string);
                            let source = v
                                .get("source")
                                .and_then(|s| s.as_str())
                                .map(str::to_string);
                            let page = v
                                .get("page")
                                .and_then(|p| p.as_str())
                                .map(str::to_string);
                            let _ = tx_ipc.send(BrowserMessage::Candidate {
                                url,
                                mime,
                                source,
                                page,
                            });
                        }
                    }
                }
            })
            .with_navigation_handler(move |url| {
                let ok = url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("about:")
                    || url.starts_with("data:");
                if !ok && block_redirects {
                    return false;
                }
                if url.starts_with("http://") || url.starts_with("https://") {
                    let _ = tx_nav.send(BrowserMessage::PageUrl(url));
                }
                true
            })
            .with_on_page_load_handler(move |_event, url| {
                // Skip about:blank & co: the URL bar now mirrors these messages.
                if url.starts_with("http://") || url.starts_with("https://") {
                    let _ = tx_load.send(BrowserMessage::PageUrl(url));
                }
            })
            .with_bounds(egui_to_wry_rect(bounds));

        // Linux: build_as_child is X11-only — see linux_webview.rs (CachyOS Wayland).
        let webview = builder.build_as_child(parent).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("window handle kind") {
                anyhow::anyhow!(
                    "webview: {msg}. On Linux the in-app browser needs an X11 window \
                     (wry child WebView is X11-only). StreamCraft forces \
                     WINIT_UNIX_BACKEND=x11 at startup; if you overrode it to wayland, \
                     unset it or run under X11/XWayland."
                )
            } else {
                anyhow::anyhow!("webview: {e}")
            }
        })?;

        // Parité Tauri/Swift: WKScriptMessageHandler all-frames (pas seulement wry ipc).
        // wry.manager() == Tauri WKWebView.configuration.userContentController
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            match macos_media_hook::attach(&webview.manager(), pending_media.clone()) {
                Some(_) => tracing::info!("streamcraftMedia WKScriptMessageHandler attached"),
                None => tracing::error!("streamcraftMedia attach failed"),
            }
            // Tauri `spawn_media_poller`: do not rely on egui frames (paused when occluded).
            let pending_poll = pending_media.clone();
            let tx_poll = tx.clone();
            std::thread::Builder::new()
                .name("streamcraft-media-poll".into())
                .spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let batch = pending_poll
                        .lock()
                        .map(|mut g| g.drain(..).collect::<Vec<_>>())
                        .unwrap_or_default();
                    for p in batch {
                        if tx_poll
                            .send(BrowserMessage::Candidate {
                                url: p.url,
                                mime: p.mime,
                                source: p.source,
                                page: p.page,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
                .ok();
        }

        Ok(Self {
            webview,
            #[cfg(target_os = "macos")]
            pending_media,
        })
    }

    /// Drain native WKScriptMessageHandler payloads (all frames) — Tauri `spawn_media_poller`.
    pub fn take_pending_media(&self) -> Vec<BrowserMessage> {
        #[cfg(target_os = "macos")]
        {
            let batch = self
                .pending_media
                .lock()
                .map(|mut g| g.drain(..).collect::<Vec<_>>())
                .unwrap_or_default();
            return batch
                .into_iter()
                .map(|p| BrowserMessage::Candidate {
                    url: p.url,
                    mime: p.mime,
                    source: p.source,
                    page: p.page,
                })
                .collect();
        }
        #[cfg(not(target_os = "macos"))]
        Vec::new()
    }

    pub fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.webview
            .load_url(url)
            .map_err(|e| anyhow::anyhow!("load: {e}"))
    }

    /// Page history (wry has no back/forward API).
    pub fn history_step(&self, back: bool) -> anyhow::Result<()> {
        self.webview
            .evaluate_script(if back { "history.back()" } else { "history.forward()" })
            .map_err(|e| anyhow::anyhow!("history: {e}"))
    }

    pub fn set_bounds(&self, bounds: egui::Rect) -> anyhow::Result<()> {
        self.webview
            .set_bounds(egui_to_wry_rect(bounds))
            .map_err(|e| anyhow::anyhow!("bounds: {e}"))
    }

    pub fn set_visible(&self, visible: bool) -> anyhow::Result<()> {
        self.webview
            .set_visible(visible)
            .map_err(|e| anyhow::anyhow!("visible: {e}"))
    }

    pub fn rescan(&self) -> anyhow::Result<()> {
        self.webview
            .evaluate_script(RESCAN_JS)
            .map_err(|e| anyhow::anyhow!("rescan: {e}"))?;
        self.drain_queue();
        Ok(())
    }

    /// Flush JS media queue via wry ipc (top-frame). Does **not** call
    /// `DRAIN_MEDIA_QUEUE_JS` first — that script empties the array and would
    /// drop payloads before this emit (Rescan / non-WK fallback).
    pub fn drain_queue(&self) {
        let _ = self.webview.evaluate_script(
            r#"(function(){
  try {
    var q = window.__streamcraftMediaQueue || [];
    if (!q.length) return;
    // Keep the same array object so IPC_BRIDGE_JS push wrapper stays attached.
    var copy = q.slice();
    q.length = 0;
    for (var i = 0; i < copy.length; i++) {
      var p = copy[i] || {};
      window.ipc.postMessage(JSON.stringify({
        type:'media', url:String(p.url||''), mime:p.mime||null,
        source:p.source||'drain', page:p.page||location.href
      }));
    }
  } catch(e) {}
})();"#,
        );
    }

    pub fn cookie_snapshot(&self, page_url: Option<&str>) -> SessionCredentialSnapshot {
        let cookies = if let Some(u) = page_url {
            self.webview.cookies_for_url(u).unwrap_or_default()
        } else {
            self.webview.cookies().unwrap_or_default()
        };
        SessionCredentialSnapshot {
            cookies: cookies
                .into_iter()
                .map(|c| CookieEntry {
                    name: c.name().to_string(),
                    value: c.value().to_string(),
                    domain: c.domain().unwrap_or("").to_string(),
                    path: c.path().unwrap_or("/").to_string(),
                })
                .collect(),
        }
    }
}

/// `rect` is in physical pixels (egui points × pixels_per_point).
fn egui_to_wry_rect(rect: egui::Rect) -> Rect {
    Rect {
        position: PhysicalPosition::new(rect.min.x.round() as i32, rect.min.y.round() as i32).into(),
        size: PhysicalSize::new(rect.width().round() as u32, rect.height().round() as u32).into(),
    }
}
