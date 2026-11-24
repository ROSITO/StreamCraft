//! Season / episode enumeration helpers.

use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRef {
    pub season: u32,
    pub episode: u32,
    pub title: Option<String>,
    pub url: Option<String>,
}

pub fn enumerate_from_html(html: &str, page_url: &Url) -> Vec<EpisodeRef> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Pattern /tv/{id}/s/{S}/e/{E}
    let re = Regex::new(r"(?i)/tv/[^/]+/s/(\d+)/e/(\d+)").unwrap();
    for caps in re.captures_iter(html) {
        let season: u32 = caps[1].parse().unwrap_or(0);
        let episode: u32 = caps[2].parse().unwrap_or(0);
        if season == 0 || episode == 0 {
            continue;
        }
        if !seen.insert((season, episode)) {
            continue;
        }
        let mut url = None;
        if let Some(m) = caps.get(0) {
            if let Ok(u) = page_url.join(m.as_str()) {
                url = Some(u.to_string());
            }
        }
        out.push(EpisodeRef {
            season,
            episode,
            title: None,
            url,
        });
    }

    // Catalogue style eps_*.txt lines: S01E02|Title|url
    let line_re = Regex::new(r"(?i)S(\d+)E(\d+)\s*[|\t]\s*([^|\n]+)(?:[|\t]\s*(\S+))?").unwrap();
    for caps in line_re.captures_iter(html) {
        let season: u32 = caps[1].parse().unwrap_or(0);
        let episode: u32 = caps[2].parse().unwrap_or(0);
        if season == 0 || episode == 0 || !seen.insert((season, episode)) {
            continue;
        }
        out.push(EpisodeRef {
            season,
            episode,
            title: Some(caps[3].trim().to_string()),
            url: caps.get(4).map(|m| m.as_str().to_string()),
        });
    }

    out.sort_by_key(|e| (e.season, e.episode));
    out
}
