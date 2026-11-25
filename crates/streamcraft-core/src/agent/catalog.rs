//! Episode catalogue (`/data/eps_{id}.txt`) + embed → HLS extraction.
//! Mirrors the site player bootstrap used by the archived Swift season path.

use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::media::heuristics::MediaURLHeuristics;
use crate::media::models::NetworkMediaCandidate;

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone)]
pub struct EpisodePlayers {
    pub version: String,
    pub episode: u32,
    pub title: Option<String>,
    pub preferred_embed: Option<Url>,
    pub embeds: Vec<(String, Url)>,
}

pub struct SeriesCatalog {
    pub news_id: String,
    pub episodes: Vec<EpisodePlayers>,
}

const PLAYER_PREFERENCE: &[&str] = &["vidzy", "premium", "voe", "uqload", "netu"];

pub fn extract_news_id(html: &str, page_url: &Url) -> Option<String> {
    let patterns = [
        r#"(?i)data-newsid=["'](\d+)["']"#,
        r#"(?i)dle_news_id\s*=\s*['"](\d+)['"]"#,
        r#"(?i)[?&]newsid=(\d+)"#,
        r#"(?i)/(\d{6,})-[a-z0-9\-]+\.html"#,
    ];
    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(html) {
                return Some(caps[1].to_string());
            }
            if let Some(caps) = re.captures(page_url.as_str()) {
                return Some(caps[1].to_string());
            }
        }
    }
    None
}

pub async fn fetch_series_catalog(
    client: &Client,
    page_url: &Url,
    news_id: &str,
) -> Result<SeriesCatalog, CatalogError> {
    let origin = origin_of(page_url)?;
    let paths = [
        format!("/static/series/{news_id}.js"),
        format!("/data/eps_{news_id}.txt"),
        format!("/ep-data.php?id={news_id}&format=js"),
    ];

    let mut last_err = CatalogError::Message("no catalogue path succeeded".into());
    for path in paths {
        let url = format!("{origin}{path}");
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.text().await {
                    Ok(body) => match parse_eps_json(&body) {
                        Ok(episodes) if !episodes.is_empty() => {
                            return Ok(SeriesCatalog {
                                news_id: news_id.to_string(),
                                episodes,
                            });
                        }
                        Ok(_) => {
                            last_err = CatalogError::Message(format!("{path}: empty catalogue"));
                        }
                        Err(e) => last_err = e,
                    },
                    Err(e) => last_err = CatalogError::Http(e),
                }
            }
            Ok(resp) => {
                last_err = CatalogError::Message(format!("{path}: HTTP {}", resp.status()));
            }
            Err(e) => last_err = CatalogError::Http(e),
        }
    }
    Err(last_err)
}

fn parse_eps_json(body: &str) -> Result<Vec<EpisodePlayers>, CatalogError> {
    let trimmed = body.trim();
    // Sometimes served as JS assignment; try to locate JSON object.
    let json_text = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else if let Some(idx) = trimmed.find('{') {
        trimmed[idx..].to_string()
    } else {
        return Err(CatalogError::Message("catalogue is not JSON".into()));
    };

    let value: Value = serde_json::from_str(&json_text)
        .map_err(|e| CatalogError::Message(format!("catalogue JSON: {e}")))?;
    let info = value.get("info").cloned().unwrap_or(Value::Null);

    let mut out = Vec::new();
    for version in ["vf", "vostfr", "vo"] {
        let Some(map) = value.get(version).and_then(|v| v.as_object()) else {
            continue;
        };
        for (k, ep) in map {
            let Ok(num) = k.parse::<u32>() else { continue };
            let Some(obj) = ep.as_object() else { continue };
            let mut embeds = Vec::new();
            for (player, raw) in obj {
                if let Some(s) = raw.as_str() {
                    if let Ok(u) = Url::parse(s) {
                        if matches!(u.scheme(), "http" | "https") {
                            embeds.push((player.to_lowercase(), u));
                        }
                    }
                }
            }
            if embeds.is_empty() {
                continue;
            }
            let preferred = PLAYER_PREFERENCE
                .iter()
                .find_map(|p| embeds.iter().find(|(n, _)| n == p).map(|(_, u)| u.clone()))
                .or_else(|| embeds.first().map(|(_, u)| u.clone()));

            let title = info
                .get(k)
                .and_then(|v| {
                    v.get("title")
                        .or_else(|| v.get("name"))
                        .and_then(|x| x.as_str())
                })
                .map(|s| s.to_string());

            out.push(EpisodePlayers {
                version: version.to_string(),
                episode: num,
                title,
                preferred_embed: preferred,
                embeds,
            });
        }
    }
    out.sort_by_key(|e| (e.version.clone(), e.episode));
    Ok(out)
}

/// Extract playable media URLs from an embed page (Vidzy / similar).
pub fn extract_stream_urls_from_embed(html: &str) -> Vec<Url> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let patterns = [
        r#"(?i)sources?\s*:\s*\[\s*\{\s*src\s*:\s*"([^"]+\.m3u8[^"]*)""#,
        r#"(?i)sources?\s*:\s*\[\s*\{\s*src\s*:\s*'([^']+\.m3u8[^']*)'"#,
        r#"(?i)file\s*:\s*"([^"]+\.m3u8[^"]*)""#,
        r#"(?i)src\s*:\s*"([^"]+\.m3u8[^"]*)""#,
        r#"(https?://[^"'\s<>]+\.m3u8[^"'\s<>]*)"#,
    ];

    for pat in patterns {
        let Ok(re) = Regex::new(pat) else { continue };
        for caps in re.captures_iter(html) {
            if let Some(m) = caps.get(1) {
                let raw = m.as_str().replace("\\/", "/");
                if is_troll_stream(&raw) {
                    continue;
                }
                if let Ok(url) = Url::parse(&raw) {
                    if MediaURLHeuristics::looks_like_media_core(&url, Some("application/vnd.apple.mpegurl"))
                        && seen.insert(url.as_str().to_string())
                    {
                        out.push(url);
                    }
                }
            }
        }
    }
    out
}

fn is_troll_stream(url: &str) -> bool {
    let u = url.to_lowercase();
    u.contains("/troll/") || u.contains("troll/master.m3u8")
}

pub async fn resolve_embed_streams(
    client: &Client,
    embed: &Url,
    referer: &Url,
) -> Result<Vec<NetworkMediaCandidate>, CatalogError> {
    let html = client
        .get(embed.clone())
        .header("Referer", referer.as_str())
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let mut out = Vec::new();
    for url in extract_stream_urls_from_embed(&html) {
        let mut c = NetworkMediaCandidate::new(url, "embed");
        c.page_url = Some(embed.clone());
        c.mime_type = Some("application/vnd.apple.mpegurl".into());
        out.push(c);
    }
    Ok(out)
}

fn origin_of(url: &Url) -> Result<String, CatalogError> {
    let host = url
        .host_str()
        .ok_or_else(|| CatalogError::Message("page URL has no host".into()))?;
    let scheme = url.scheme();
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(format!("{scheme}://{host}{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_news_id_from_slug() {
        let url = Url::parse("https://example.com/15140968-visionquest-saison-1-2026.html").unwrap();
        let id = extract_news_id("", &url).unwrap();
        assert_eq!(id, "15140968");
    }

    #[test]
    fn filters_troll_and_keeps_real_m3u8() {
        let html = r#"
        var _fsvHls="https://s1.fsvid.lol/troll/master.m3u8";
        sources: [{src: "https://u14.vidzy.org/hls2/08/00064/master.m3u8?t=abc", type: "application/x-mpegURL"}]
        "#;
        let urls = extract_stream_urls_from_embed(html);
        assert_eq!(urls.len(), 1);
        assert!(urls[0].as_str().contains("u14.vidzy.org"));
    }

    #[test]
    fn parses_eps_json() {
        let body = r#"{"vf":{"1":{"vidzy":"https://vidzy.org/embed-abc.html","uqload":"https://uqload.vc/x.html"}},"vostfr":{},"vo":{},"info":{"1":{"title":"Pilot"}}}"#;
        let eps = parse_eps_json(body).unwrap();
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].episode, 1);
        assert_eq!(eps[0].version, "vf");
        assert!(eps[0]
            .preferred_embed
            .as_ref()
            .unwrap()
            .as_str()
            .contains("vidzy.org"));
    }
}
