//! CDN gate tokens (e.g. Vidzy `gk=`) observed from the in-app browser player.
//!
//! Cold master playlists omit `gk`. Segments 1–9 may still 200; from ~seg 10 the
//! CDN returns 403. The player unlocks media playlists with `gk=…` — capture that
//! URL (PopupGuard) and prefer / propagate it. Not DRM bypass: same token the
//! browser already received for the user's session.
//!
//! Callers: `download/planner.rs` (append token onto segment URIs),
//! `download/service.rs` (propagate onto variant/audio before plan),
//! `apps/streamcraft` detect ranking (prefer unlocked media over cold master).

use url::Url;

const GATE_KEYS: &[&str] = &["gk"];

/// Extract the first known gate token (`gk`) from a URL query string.
pub fn gate_token(url: &Url) -> Option<String> {
    for (k, v) in url.query_pairs() {
        if GATE_KEYS.iter().any(|g| k.eq_ignore_ascii_case(g)) && !v.is_empty() {
            return Some(v.into_owned());
        }
    }
    None
}

pub fn has_gate_token(url: &Url) -> bool {
    gate_token(url).is_some()
}

/// Host + path (no query/fragment) for same-asset matching across token variants.
pub fn asset_key(url: &Url) -> String {
    format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or(""),
        url.path()
    )
}

/// Same CDN directory (parent of playlist/segment file), ignoring query tokens.
pub fn same_stream_family(a: &Url, b: &Url) -> bool {
    if a.host_str() != b.host_str() {
        return false;
    }
    let pa = dirname(a.path());
    let pb = dirname(b.path());
    !pa.is_empty() && pa == pb
}

fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or(path)
}

/// Ensure `url` carries `gk=<token>` when missing.
/// Append-only: never re-encodes existing query bytes (CDN signed `t`/`s`/`e`).
pub fn apply_gate_token(url: &Url, token: &str) -> Url {
    if token.is_empty() || has_gate_token(url) {
        return url.clone();
    }
    let mut out = url.clone();
    let raw = url.query().unwrap_or("");
    let sep = if raw.is_empty() { "" } else { "&" };
    // Token is opaque CDN material; keep unreserved + `.` unescaped like observed URLs.
    let encoded: String = token
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let q = format!("{raw}{sep}gk={encoded}");
    out.set_query(Some(&q));
    out
}

/// Prefer unlocked media playlists over cold masters when ranking detections.
pub fn hls_capture_score(url: &Url) -> i32 {
    let u = url.as_str().to_lowercase();
    let mut s = 0;
    if has_gate_token(url) {
        s += 100;
    }
    if u.contains("index-v") || u.contains("index-f") {
        s += 40;
    } else if u.contains("master.m3u8") || u.ends_with("master") {
        s -= 20;
    }
    if u.contains("index-a") && !u.contains("index-v") {
        s -= 10;
    }
    s
}

fn looks_gated_cdn(url: &Url) -> bool {
    let u = url.as_str().to_lowercase();
    u.contains("urlset") || u.contains("vidzy") || u.contains("/hls2/")
}

/// True when this gated CDN HLS URL still needs a player-unlocked `gk`.
pub fn needs_player_unlock(url: &Url) -> bool {
    looks_gated_cdn(url) && !has_gate_token(url)
}

/// Best `gk` among `urls`, optionally restricted to the same CDN family as `anchor`.
pub fn best_gate_token<'a, I>(urls: I) -> Option<String>
where
    I: IntoIterator<Item = &'a Url>,
{
    best_gate_token_for(None, urls)
}

pub fn best_gate_token_for<'a, I>(anchor: Option<&Url>, urls: I) -> Option<String>
where
    I: IntoIterator<Item = &'a Url>,
{
    urls.into_iter().find_map(|u| {
        if let Some(a) = anchor {
            if !same_stream_family(a, u) {
                return None;
            }
        }
        gate_token(u)
    })
}

/// Vidzy-style demuxed video playlist (`index-v1-a2.m3u8`) — sibling audio names.
/// Returns `(playlist_url, language, name, is_default)` with FR first / default.
pub fn suggest_urlset_audio_siblings(
    video_playlist: &Url,
) -> Vec<(Url, &'static str, &'static str, bool)> {
    let path = video_playlist.path().to_lowercase();
    let file = path.rsplit('/').next().unwrap_or("");
    if !(file.contains("index-v") || file.starts_with("index-f")) {
        return Vec::new();
    }
    if file.contains("index-a") && !file.contains("index-v") {
        return Vec::new();
    }
    let parent = dirname(video_playlist.path());
    let query = video_playlist.query().map(|q| q.to_string());
    let mk = |name: &str| -> Option<Url> {
        let mut u = video_playlist.clone();
        let new_path = if parent.is_empty() {
            format!("/{name}.m3u8")
        } else {
            format!("{parent}/{name}.m3u8")
        };
        u.set_path(&new_path);
        if let Some(ref q) = query {
            u.set_query(Some(q));
        }
        Some(u)
    };
    let mut out = Vec::new();
    if let Some(fr) = mk("index-a1") {
        out.push((fr, "fr", "Français", true));
    }
    if let Some(en) = mk("index-a2") {
        out.push((en, "en", "English", false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_roundtrip_preserves_raw_query() {
        let cold: Url =
            "https://u14.vidzy.org/hls2/x/urlset/seg-10.ts?t=ab+c&s=1&e=2&fr=id"
                .parse()
                .unwrap();
        assert!(gate_token(&cold).is_none());
        let hot = apply_gate_token(&cold, "tok.123");
        assert_eq!(gate_token(&hot).as_deref(), Some("tok.123"));
        // Append-only: original query bytes (incl. `+`) stay intact.
        assert!(hot.as_str().contains("t=ab+c&s=1&e=2&fr=id&gk=tok.123"));
        assert!(has_gate_token(&hot));
        // Already gated → unchanged.
        assert_eq!(apply_gate_token(&hot, "other").as_str(), hot.as_str());
    }

    #[test]
    fn family_and_score() {
        let master: Url =
            "https://u14.vidzy.org/hls2/06/00061/,e94,.urlset/master.m3u8?t=1"
                .parse()
                .unwrap();
        let cold_media: Url =
            "https://u14.vidzy.org/hls2/06/00061/,e94,.urlset/index-v1-a2.m3u8?t=1"
                .parse()
                .unwrap();
        let media: Url =
            "https://u14.vidzy.org/hls2/06/00061/,e94,.urlset/index-v1-a2.m3u8?t=1&gk=xyz"
                .parse()
                .unwrap();
        assert!(same_stream_family(&master, &media));
        assert!(needs_player_unlock(&master));
        assert!(needs_player_unlock(&cold_media));
        assert!(!needs_player_unlock(&media));
        assert!(hls_capture_score(&media) > hls_capture_score(&master));
    }

    #[test]
    fn suggests_fr_audio_sibling() {
        let media: Url =
            "https://u14.vidzy.org/hls2/x/,id,.urlset/index-v1-a2.m3u8?t=1&gk=tok"
                .parse()
                .unwrap();
        let sibs = suggest_urlset_audio_siblings(&media);
        assert_eq!(sibs.len(), 2);
        assert_eq!(sibs[0].1, "fr");
        assert!(sibs[0].3);
        assert!(sibs[0].0.as_str().contains("index-a1.m3u8"));
        assert!(sibs[0].0.as_str().contains("gk=tok"));
    }
}
