use url::Url;

pub struct PopupGuard;

impl PopupGuard {
    pub fn same_site(current: Option<&Url>, target: &Url) -> bool {
        let Some(current) = current else { return false };
        let Some(current_host) = current.host_str().map(|h| h.to_lowercase()) else {
            return false;
        };
        let Some(target_host) = target.host_str().map(|h| h.to_lowercase()) else {
            return false;
        };
        if current_host.is_empty() || target_host.is_empty() {
            return false;
        }
        if current_host == target_host {
            return true;
        }
        target_host.ends_with(&format!(".{current_host}"))
            || current_host.ends_with(&format!(".{target_host}"))
    }

    pub fn is_http_url(url: Option<&Url>) -> bool {
        matches!(url.and_then(|u| Some(u.scheme())), Some("http" | "https"))
    }

    pub fn should_open_blank_in_place(
        current_page: Option<&Url>,
        target: Option<&Url>,
        user_activated: bool,
    ) -> bool {
        if !user_activated {
            return false;
        }
        let Some(target) = target else { return false };
        Self::is_http_url(Some(target)) && Self::same_site(current_page, target)
    }

    pub const NEUTRALIZE_WINDOW_OPEN_JS: &'static str = r#"(function () {
  try {
    window.open = function () { return null; };
    Object.defineProperty(window, 'open', {
      configurable: true,
      writable: true,
      value: function () { return null; }
    });
  } catch (e) {}
})();"#;

    /// Network observer (D008) — injected in all frames by wry.
    /// Callers: `apps/streamcraft/src/browser/mod.rs` (`with_initialization_script_for_main_only(..., false)`).
    /// wry `window.ipc` is top-frame only; iframe hits bubble via `postMessage({__streamcraftMedia})`.
    /// User: "t'as juste prouvé qu'en entrant le lien précis du player… je met ce lien, rien ne se passe"
    pub const MEDIA_HOOKS_JS: &'static str = r#"(function () {
  if (window.__streamcraftMediaHookInstalled) { return; }
  window.__streamcraftMediaHookInstalled = true;
  window.__streamcraftMediaQueue = window.__streamcraftMediaQueue || [];

  try { window.open = function () { return null; }; } catch (e) {}

  var isTop = false;
  try { isTop = (window === window.top); } catch (eTop) { isTop = false; }

  function deliverPayload(payload) {
    try {
      if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.streamcraftMedia) {
        window.webkit.messageHandlers.streamcraftMedia.postMessage(payload);
        return;
      }
    } catch (e0) {}
    try {
      if (window.ipc && typeof window.ipc.postMessage === 'function') {
        window.ipc.postMessage(JSON.stringify({
          type: 'media',
          url: String(payload.url || ''),
          mime: payload.mime || null,
          source: payload.source || 'hook',
          page: payload.page || (location && location.href) || null
        }));
        return;
      }
    } catch (e1) {}
    try {
      if (!isTop && window.top) {
        window.top.postMessage({ __streamcraftMedia: true, payload: payload }, '*');
        return;
      }
    } catch (e2) {}
    try {
      if (!isTop && window.parent && window.parent !== window) {
        window.parent.postMessage({ __streamcraftMedia: true, payload: payload }, '*');
        return;
      }
    } catch (e3) {}
    window.__streamcraftMediaQueue = window.__streamcraftMediaQueue || [];
    window.__streamcraftMediaQueue.push(payload);
  }

  function post(url, mime, source) {
    try {
      if (!url || typeof url !== 'string') { return; }
      if (url.indexOf('blob:') === 0 || url.indexOf('data:') === 0) { return; }
      deliverPayload({
        url: url,
        mime: mime || null,
        source: source || 'resourceTiming',
        page: location.href
      });
    } catch (e) {}
  }

  function maybeMedia(url, mime) {
    if (!url) { return; }
    var lower = String(url).toLowerCase();
    var m = (mime || '').toLowerCase();
    if (
      lower.indexOf('.m3u8') !== -1 ||
      lower.indexOf('.mpd') !== -1 ||
      lower.indexOf('.mp4') !== -1 ||
      lower.indexOf('.m4s') !== -1 ||
      lower.indexOf('.ts') !== -1 ||
      lower.indexOf('m3u8') !== -1 ||
      lower.indexOf('/embed-') !== -1 ||
      lower.indexOf('/embed/') !== -1 ||
      lower.indexOf('/iframe/') !== -1 ||
      lower.indexOf('vidzy.org') !== -1 ||
      lower.indexOf('sharecloudy') !== -1 ||
      lower.indexOf('urlset') !== -1 ||
      m.indexOf('mpegurl') !== -1 ||
      m.indexOf('dash+xml') !== -1 ||
      m.indexOf('video/') === 0 ||
      m.indexOf('application/vnd.apple.mpegurl') !== -1
    ) {
      return true;
    }
    return false;
  }

  var seen = {};
  function consider(url, mime, source) {
    if (!url || seen[url]) { return; }
    if (!maybeMedia(url, mime)) { return; }
    seen[url] = true;
    post(url, mime, source);
  }

  function flushLocalQueue() {
    try {
      var q = window.__streamcraftMediaQueue || [];
      window.__streamcraftMediaQueue = [];
      for (var i = 0; i < q.length; i++) {
        deliverPayload(q[i] || {});
      }
    } catch (ef) {}
  }

  function scanResources() {
    try {
      var entries = performance.getEntriesByType('resource');
      for (var i = 0; i < entries.length; i++) {
        consider(entries[i].name, null, 'resourceTiming');
      }
    } catch (err) {}
  }

  function scanIframes() {
    try {
      var frames = document.querySelectorAll('iframe');
      for (var i = 0; i < frames.length; i++) {
        var f = frames[i];
        var u = f.src || f.getAttribute('src') || f.getAttribute('data-src');
        if (u) { consider(u, null, 'iframeSrc'); }
        try {
          if (f.contentWindow) {
            f.contentWindow.postMessage({ __streamcraftFlush: true }, '*');
          }
        } catch (ef2) {}
      }
    } catch (ei) {}
  }

  setInterval(function () {
    scanResources();
    if (isTop) { scanIframes(); }
    flushLocalQueue();
  }, 1000);
  scanResources();
  if (isTop) { scanIframes(); }

  if (isTop) {
    window.addEventListener('message', function (ev) {
      try {
        var d = ev && ev.data;
        if (!d || !d.__streamcraftMedia || !d.payload) { return; }
        deliverPayload(d.payload);
      } catch (em) {}
    }, false);
  }

  window.addEventListener('message', function (ev) {
    try {
      var d = ev && ev.data;
      if (!d || !d.__streamcraftFlush) { return; }
      scanResources();
      flushLocalQueue();
    } catch (ef3) {}
  }, false);

  if (window.fetch) {
    var originalFetch = window.fetch;
    window.fetch = function () {
      try {
        var input = arguments[0];
        var url = typeof input === 'string' ? input : (input && input.url);
        if (url) { consider(url, null, 'fetchHook'); }
      } catch (e) {}
      return originalFetch.apply(this, arguments).then(function (response) {
        try {
          var ct = response.headers && response.headers.get && response.headers.get('content-type');
          consider(response.url, ct, 'fetchHook');
        } catch (e2) {}
        return response;
      });
    };
  }

  var OriginalXHR = window.XMLHttpRequest;
  if (OriginalXHR) {
    window.XMLHttpRequest = function () {
      var xhr = new OriginalXHR();
      var open = xhr.open;
      xhr.open = function (method, url) {
        try { consider(url, null, 'xhrHook'); } catch (e) {}
        return open.apply(xhr, arguments);
      };
      return xhr;
    };
  }

  function watchMedia(el) {
    try {
      if (el && el.src) { consider(el.src, el.type || null, 'mediaElement'); }
      if (el && el.currentSrc) { consider(el.currentSrc, el.type || null, 'mediaElement'); }
    } catch (e) {}
  }
  document.querySelectorAll('video, audio, source').forEach(watchMedia);
  var mo = new MutationObserver(function (mutations) {
    mutations.forEach(function (m) {
      m.addedNodes && m.addedNodes.forEach(function (n) {
        if (n.tagName === 'VIDEO' || n.tagName === 'AUDIO' || n.tagName === 'SOURCE') {
          watchMedia(n);
        }
        if (n.tagName === 'IFRAME') {
          var iu = n.src || (n.getAttribute && n.getAttribute('src'));
          if (iu) { consider(iu, null, 'iframeSrc'); }
        }
        if (n.querySelectorAll) {
          n.querySelectorAll('video, audio, source').forEach(watchMedia);
          n.querySelectorAll('iframe').forEach(function (f) {
            var u2 = f.src || f.getAttribute('src');
            if (u2) { consider(u2, null, 'iframeSrc'); }
          });
        }
      });
      if (m.type === 'attributes' && m.target && m.target.tagName === 'IFRAME') {
        var au = m.target.src || m.target.getAttribute('src');
        if (au) { consider(au, null, 'iframeSrc'); }
      }
    });
  });
  mo.observe(document.documentElement || document, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ['src', 'data-src']
  });
})();"#;

    pub const DRAIN_MEDIA_QUEUE_JS: &'static str = r#"(function () {
  var q = window.__streamcraftMediaQueue || [];
  window.__streamcraftMediaQueue = [];
  return JSON.stringify(q);
})()"#;

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_site_subdomain() {
        let a = Url::parse("https://example.com/a").unwrap();
        let b = Url::parse("https://cdn.example.com/b").unwrap();
        assert!(PopupGuard::same_site(Some(&a), &b));
        let c = Url::parse("https://other.com/x").unwrap();
        assert!(!PopupGuard::same_site(Some(&a), &c));
    }
}
