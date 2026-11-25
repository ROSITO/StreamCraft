use url::Url;

pub struct MediaURLHeuristics;

impl MediaURLHeuristics {
    const MEDIA_EXTENSIONS: &'static [&'static str] =
        &["m3u8", "mpd", "mp4", "m4v", "m4s", "ts", "webm", "mov"];
    const MEDIA_PATH_TOKENS: &'static [&'static str] =
        &["m3u8", "mpd", ".mp4", "manifest", "playlist"];

    /// Player embed pages that usually hide the real m3u8 until scraped / played.
    /// Callers: `detect::ingest_candidate`, PopupGuard `maybeMedia` (browser).
    /// User: rizmov page → Detected empty (embed iframe not bubbled).
    const EMBED_HOST_TOKENS: &'static [&'static str] = &[
        "vidzy.org",
        "vidzy.",
        "sharecloudy.com",
        "sharecloudy.",
        "dood.",
        "streamtape.",
        "filemoon.",
        "voe.sx",
        "upstream.to",
    ];

    pub fn looks_like_media(url: &Url, mime_type: Option<&str>) -> bool {
        Self::looks_like_media_core(url, mime_type)
    }

    pub fn looks_like_embed_player(url: &Url) -> bool {
        let hay = url.as_str().to_lowercase();
        let path = url.path().to_lowercase();
        if path.contains("/embed-")
            || path.contains("/embed/")
            || path.contains("embed-")
            || path.contains("/iframe/")
        {
            return true;
        }
        let host_hit = Self::EMBED_HOST_TOKENS.iter().any(|t| hay.contains(t));
        host_hit
            && (path.contains("embed")
                || path.contains("iframe")
                || hay.contains("embed")
                || hay.contains("iframe"))
    }

    pub fn looks_like_media_core(url: &Url, mime_type: Option<&str>) -> bool {
        if let Some(mime) = mime_type {
            if Self::mime_looks_like_media(mime) {
                return true;
            }
        }
        if Self::looks_like_embed_player(url) {
            return true;
        }
        let ext = url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .and_then(|name| name.rsplit_once('.'))
            .map(|(_, e)| e.to_lowercase())
            .unwrap_or_default();
        if Self::MEDIA_EXTENSIONS.iter().any(|e| *e == ext) {
            return true;
        }
        let haystack = url.as_str().to_lowercase();
        Self::MEDIA_PATH_TOKENS
            .iter()
            .any(|token| haystack.contains(token))
    }

    pub fn mime_looks_like_media(mime: &str) -> bool {
        let m = mime.to_lowercase();
        m.starts_with("video/")
            || m.contains("mpegurl")
            || m.contains("dash+xml")
            || m.contains("application/vnd.apple.mpegurl")
            || m.contains("application/mp4")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_m3u8_and_mp4() {
        let m3u8 = Url::parse("https://cdn.example.com/a.m3u8").unwrap();
        let mp4 = Url::parse("https://cdn.example.com/a.mp4").unwrap();
        assert!(MediaURLHeuristics::looks_like_media_core(&m3u8, None));
        assert!(MediaURLHeuristics::looks_like_media_core(&mp4, None));
    }
}
