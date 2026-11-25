//! Headless page scrape (CLI path without WebView).

use regex::Regex;
use reqwest::Client;
use thiserror::Error;
use url::Url;

use crate::media::heuristics::MediaURLHeuristics;
use crate::media::models::NetworkMediaCandidate;

#[derive(Debug, Error)]
pub enum ScrapeError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("invalid url: {0}")]
    Url(String),
}

pub struct PageScraper {
    client: Client,
}

impl PageScraper {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
                .build()
                .expect("reqwest client"),
        }
    }

    pub async fn fetch_html(&self, page_url: &Url) -> Result<String, ScrapeError> {
        let resp = self.client.get(page_url.clone()).send().await?;
        let text = resp.error_for_status()?.text().await?;
        Ok(text)
    }

    pub fn extract_candidates(html: &str, page_url: &Url) -> Vec<NetworkMediaCandidate> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let re = Regex::new(r#"(?i)https?://[^\s"'<>]+"#).unwrap();
        for m in re.find_iter(html) {
            let raw = m.as_str().trim_end_matches(['"', '\'', ')', ']', '>', ',']);
            if let Ok(url) = Url::parse(raw) {
                if MediaURLHeuristics::looks_like_media_core(&url, None) && seen.insert(url.as_str().to_string())
                {
                    let mut c = NetworkMediaCandidate::new(url, "scrape");
                    c.page_url = Some(page_url.clone());
                    out.push(c);
                }
            } else if let Ok(url) = page_url.join(raw) {
                if MediaURLHeuristics::looks_like_media_core(&url, None) && seen.insert(url.as_str().to_string())
                {
                    let mut c = NetworkMediaCandidate::new(url, "scrape");
                    c.page_url = Some(page_url.clone());
                    out.push(c);
                }
            }
        }
        // relative .m3u8 / .mpd in attributes
        let rel = Regex::new(r#"(?i)(?:src|href)=["']([^"']+\.(?:m3u8|mpd|mp4)[^"']*)["']"#).unwrap();
        for caps in rel.captures_iter(html) {
            if let Some(m) = caps.get(1) {
                if let Ok(url) = page_url.join(m.as_str()) {
                    if seen.insert(url.as_str().to_string()) {
                        let mut c = NetworkMediaCandidate::new(url, "scrape-attr");
                        c.page_url = Some(page_url.clone());
                        out.push(c);
                    }
                }
            }
        }
        out
    }

    pub fn extract_title(html: &str) -> Option<String> {
        let re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
        let caps = re.captures(html)?;
        let title = caps.get(1)?.as_str();
        let cleaned = html_escape_decode(&strip_tags(title)).trim().to_string();
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned)
        }
    }
}

impl Default for PageScraper {
    fn default() -> Self {
        Self::new()
    }
}

fn strip_tags(s: &str) -> String {
    let re = Regex::new(r"<[^>]+>").unwrap();
    re.replace_all(s, "").to_string()
}

fn html_escape_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}
