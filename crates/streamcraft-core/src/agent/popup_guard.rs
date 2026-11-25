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

    /// Network observer injection — clone exact de `NetworkObserver.injectionSource` (Swift D008).
    /// `webkit.messageHandlers` → file `window.__streamcraftMediaQueue` (poll Rust).
    pub const MEDIA_HOOKS_JS: &'static str = r#"(function () {
  if (window.__streamcraftMediaHookInstalled) { return; }
  window.__streamcraftMediaHookInstalled = true;
  window.__streamcraftMediaQueue = window.__streamcraftMediaQueue || [];

  try { window.open = function () { return null; }; } catch (e) {}

  function post(url, mime, source) {
    try {
      if (!url || typeof url !== 'string') { return; }
      if (url.indexOf('blob:') === 0 || url.indexOf('data:') === 0) { return; }
      var payload = {
        url: url,
        mime: mime || null,
        source: source || 'resourceTiming',
        page: location.href
      };
      try {
        window.webkit.messageHandlers.streamcraftMedia.postMessage(payload);
      } catch (e1) {
        window.__streamcraftMediaQueue = window.__streamcraftMediaQueue || [];
        window.__streamcraftMediaQueue.push(payload);
      }
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

  function scanResources() {
    try {
      var entries = performance.getEntriesByType('resource');
      for (var i = 0; i < entries.length; i++) {
        var e = entries[i];
        consider(e.name, null, 'resourceTiming');
      }
    } catch (err) {}
  }
  setInterval(scanResources, 1000);
  scanResources();

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
        if (n.querySelectorAll) {
          n.querySelectorAll('video, audio, source').forEach(watchMedia);
        }
      });
    });
  });
  mo.observe(document.documentElement || document, { childList: true, subtree: true });
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
