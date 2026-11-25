//! PageTitleResolver — port Swift `PageTitleResolver` (titres contenu, pas marque site).

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TitleCandidate {
    pub text: String,
    pub source: String,
}

const JUNK_EXACT: &[&str] = &[
    "StreamCraft",
    "about:blank",
    "Blank",
    "New Tab",
    "Untitled",
    "Sans titre",
    "Home",
    "Accueil",
    "Watch",
    "Streaming",
    "Voir",
    "Lecture",
];

fn source_score(source: &str) -> i32 {
    match source {
        "jsonld" => 100,
        "og" => 90,
        "twitter" => 85,
        "itemprop" => 80,
        "h1" => 70,
        "player" => 65,
        "video" => 60,
        "document" => 20,
        "url" => 10,
        _ => 30,
    }
}

pub fn useful_title(raw: Option<&str>, host: Option<&str>) -> Option<String> {
    clean(raw, host).filter(|s| !s.is_empty())
}

/// Meilleur titre parmi candidats DOM + fallbacks URL.
pub fn resolve_title(
    candidates: &[TitleCandidate],
    page_title: Option<&str>,
    page_url: Option<&Url>,
    media_url: &Url,
) -> String {
    let host = page_url
        .and_then(|u| u.host_str())
        .or_else(|| media_url.host_str());

    let mut pool: Vec<TitleCandidate> = candidates.to_vec();
    if let Some(pt) = page_title {
        pool.push(TitleCandidate {
            text: pt.to_string(),
            source: "document".into(),
        });
    }
    if let Some(pu) = page_url {
        if let Some(from_page) = useful_title_from_url(pu) {
            pool.push(TitleCandidate {
                text: from_page,
                source: "url".into(),
            });
        }
    }
    if let Some(from_media) = useful_title_from_url(media_url) {
        pool.push(TitleCandidate {
            text: from_media,
            source: "url".into(),
        });
    }

    let mut ranked: Vec<(String, i32)> = pool
        .into_iter()
        .filter_map(|c| {
            let text = clean(Some(&c.text), host)?;
            if looks_like_site_brand(&text, host) {
                return None;
            }
            let mut score = source_score(&c.source);
            score += text.chars().count().min(80) as i32;
            if looks_generic_watch_word(&text) {
                score -= 40;
            }
            Some((text, score))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));

    if let Some((best, _)) = ranked.first() {
        return best.clone();
    }
    if let Some(pu) = page_url {
        if let Some(t) = useful_title_from_url(pu) {
            return t;
        }
    }
    if let Some(t) = useful_title_from_url(media_url) {
        return t;
    }
    media_url
        .host_str()
        .unwrap_or("video")
        .to_string()
}

pub fn useful_title_from_url(url: &Url) -> Option<String> {
    let host = url.host_str();
    let media_ext = [
        "m3u8", "mpd", "mp4", "m4v", "webm", "mov", "m4s", "ts", "aac", "mp3", "html", "php",
    ];
    let skip_names = [
        "watch", "movie", "film", "video", "embed", "player", "stream", "episode", "saison",
        "season", "voir",
    ];
    let parts: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    for part in parts.into_iter().rev() {
        let mut name = part.to_string();
        if let Some((stem, ext)) = name.rsplit_once('.') {
            if media_ext.contains(&ext.to_lowercase().as_str()) {
                name = stem.to_string();
            }
        }
        if looks_opaque(&name) {
            continue;
        }
        if skip_names.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        let decoded = urlencoding_loose(&name)
            .replace('_', " ")
            .replace('-', " ")
            .replace('+', " ");
        if let Some(useful) = useful_title(Some(&decoded), host) {
            if !looks_like_site_brand(&useful, host) {
                return Some(useful);
            }
        }
    }
    None
}

pub fn looks_like_site_brand(title: &str, host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let brand_tokens = brand_tokens(host);
    let title_norm = normalize_token(title);
    if title_norm.is_empty() {
        return true;
    }
    for brand in &brand_tokens {
        if title_norm == *brand {
            return true;
        }
        if title_norm.starts_with(brand) && title_norm.len() <= brand.len() + 12 {
            return true;
        }
        if brand.starts_with(&title_norm) && title_norm.len() >= 4 {
            return true;
        }
    }
    false
}

fn clean(raw: Option<&str>, host: Option<&str>) -> Option<String> {
    let mut title = raw?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    while title.contains("  ") {
        title = title.replace("  ", " ");
    }
    if JUNK_EXACT.iter().any(|j| *j == title) {
        return None;
    }
    if title.eq_ignore_ascii_case("streamcraft") {
        return None;
    }
    title = strip_site_decorations(&title, host);
    if JUNK_EXACT.iter().any(|j| *j == title) {
        return None;
    }
    if looks_like_site_brand(&title, host) {
        return None;
    }
    if looks_opaque(&title) {
        return None;
    }
    if title.chars().count() < 2 {
        return None;
    }
    Some(title.chars().take(160).collect())
}

fn strip_site_decorations(raw: &str, host: Option<&str>) -> String {
    let separators = [" — ", " – ", " - ", " | ", " · ", " • ", " / "];
    for sep in separators {
        if let Some((left, right)) = raw.split_once(sep) {
            let left = left.trim();
            let right = right.trim();
            if left.chars().count() < 2 || right.chars().count() < 2 {
                continue;
            }
            let left_brand = looks_like_site_brand(left, host) || looks_generic_watch_word(left);
            let right_brand = looks_like_site_brand(right, host) || looks_generic_watch_word(right);
            if left_brand && !right_brand {
                return right.to_string();
            }
            if right_brand && !left_brand {
                return left.to_string();
            }
            return if left.len() >= right.len() {
                left.to_string()
            } else {
                right.to_string()
            };
        }
    }
    raw.to_string()
}

fn looks_generic_watch_word(title: &str) -> bool {
    let t = title.to_lowercase();
    ["streaming", "watch online", "voir en streaming", "regarder", "free streaming"]
        .iter()
        .any(|g| t == *g || t.starts_with(g))
}

fn looks_opaque(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 3 {
        return true;
    }
    let alnum: String = t.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if alnum.len() >= 16 && alnum.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    if t.len() >= 20 && t.chars().filter(|c| c.is_ascii_digit()).count() > t.len() * 2 / 3 {
        return true;
    }
    false
}

fn brand_tokens(host: &str) -> Vec<String> {
    let host = host.to_lowercase();
    let base = host
        .strip_prefix("www.")
        .unwrap_or(&host)
        .split('.')
        .next()
        .unwrap_or(&host);
    let mut tokens = vec![normalize_token(base), normalize_token(&host)];
    tokens.retain(|t| !t.is_empty());
    tokens
}

fn normalize_token(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn urlencoding_loose(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_site_pipe() {
        let u = Url::parse("https://fs016.lol/watch/foo").unwrap();
        let t = resolve_title(
            &[],
            Some("VisionQuest S01E01 | FS016"),
            Some(&u),
            &Url::parse("https://cdn.example/a.m3u8").unwrap(),
        );
        assert!(t.to_lowercase().contains("visionquest"), "{t}");
    }

    #[test]
    fn rejects_junk_document() {
        assert!(useful_title(Some("Streaming"), Some("fs016.lol")).is_none());
    }
}
