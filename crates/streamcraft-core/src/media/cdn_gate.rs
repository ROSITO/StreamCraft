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

/// Same stream seen twice, ignoring query tokens: the same playlist file, or any
/// file of one gated per-video directory (vidzy `…/,e94,.urlset/master.m3u8` and
/// `index-v1-a2.m3u8`). A shared flat directory (`/files/aa/<id>.m3u8`, one file
/// per film) is not a family: two films there are two streams.
pub fn same_stream_family(a: &Url, b: &Url) -> bool {
    if a.host_str() != b.host_str() {
        return false;
    }
    let (pa, fa) = split_path(a.path());
    let (pb, fb) = split_path(b.path());
    !pa.is_empty() && pa == pb && (fa == fb || (looks_gated_cdn(a) && looks_gated_cdn(b)))
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or((path, ""))
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
    if is_audio_only_playlist(url) {
        // Outweighs the `gk` bonus: an audio rendition must never win over video.
        s -= 200;
    }
    s
}

/// Vidzy-style audio rendition (`index-a1.m3u8`), as opposed to `index-v1-a1`.
pub fn is_audio_only_playlist(url: &Url) -> bool {
    let u = url.path().to_lowercase();
    u.contains("index-a") && !u.contains("index-v")
}

/// Two captures of the same stream: keep the better one (never audio-only over
/// video) and give it the other's `gk` unlock if it lacks one.
/// Returns the kept media and whether it differs from `old`.
pub fn pick_capture(
    old: crate::media::models::DetectedMedia,
    new: crate::media::models::DetectedMedia,
) -> (crate::media::models::DetectedMedia, bool) {
    let old_url = old.source_url.clone();
    let (mut keep, other) = if hls_capture_score(&new.source_url) > hls_capture_score(&old.source_url) {
        (new, old)
    } else {
        (old, new)
    };
    if !has_gate_token(&keep.source_url)
        && let Some(tok) = gate_token(&other.source_url)
    {
        keep.source_url = apply_gate_token(&keep.source_url, &tok);
        for v in &mut keep.variants {
            v.playlist_url = apply_gate_token(&v.playlist_url, &tok);
        }
    }
    let changed = keep.source_url != old_url;
    (keep, changed)
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
    let parent = split_path(video_playlist.path()).0;
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
        // One flat CDN directory holding many films: different files, different streams.
        let film2: Url = "https://share86131.sharecloudy.com/files/aa/7gYQk4mul8.m3u8".parse().unwrap();
        let film3: Url = "https://share86131.sharecloudy.com/files/aa/Zq81LmPx0a.m3u8".parse().unwrap();
        assert!(!same_stream_family(&film2, &film3));
        let film2_again: Url = "https://share86131.sharecloudy.com/files/aa/7gYQk4mul8.m3u8?t=2".parse().unwrap();
        assert!(same_stream_family(&film2, &film2_again));
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

    fn media(url: &str) -> crate::media::models::DetectedMedia {
        let url: Url = url.parse().unwrap();
        crate::media::models::DetectedMedia {
            id: uuid::Uuid::new_v4(),
            source_url: url.clone(),
            kind: crate::media::models::ManifestKind::Hls,
            protection: crate::media::models::MediaProtection::None,
            suggested_title: None,
            page_url: None,
            variants: vec![crate::media::models::MediaVariant::new(url)],
            preferred_variant_id: None,
            audio_tracks: vec![],
            subtitle_tracks: vec![],
            is_vod: true,
            segment_count: None,
            duration_seconds: None,
            content_length: None,
            observed_at: chrono::Utc::now(),
        }
    }

    /// Real vidzy capture order: muxed video without `gk`, then audio-only with `gk`.
    #[test]
    fn audio_only_capture_never_replaces_video_but_lends_its_gk() {
        let base = "https://u14.vidzy.org/hls2/03/00066/,abc_n,.urlset";
        let video = media(&format!("{base}/index-v1-a1.m3u8?t=x&s=1"));
        let audio = media(&format!("{base}/index-a1.m3u8?t=x&s=1&gk=v1.tok"));
        let (keep, changed) = pick_capture(video.clone(), audio.clone());
        assert!(changed);
        assert!(keep.source_url.path().ends_with("index-v1-a1.m3u8"));
        assert_eq!(gate_token(&keep.source_url).as_deref(), Some("v1.tok"));
        assert_eq!(gate_token(&keep.variants[0].playlist_url).as_deref(), Some("v1.tok"));
        // Reverse arrival order gives the same result.
        let (keep, _) = pick_capture(audio, video);
        assert!(keep.source_url.path().ends_with("index-v1-a1.m3u8"));
        assert_eq!(gate_token(&keep.source_url).as_deref(), Some("v1.tok"));
    }
}
