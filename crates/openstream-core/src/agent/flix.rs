//! Flix digest over favorite hub RSS feeds (D019).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlixError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlixItem {
    pub id: String,
    pub title: String,
    pub link: String,
    pub published: Option<DateTime<Utc>>,
    pub hub: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlixSeenStore {
    pub seen_ids: Vec<String>,
}

impl FlixSeenStore {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), FlixError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap())?;
        Ok(())
    }

    pub fn mark(&mut self, id: &str) {
        if !self.seen_ids.iter().any(|x| x == id) {
            self.seen_ids.push(id.to_string());
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.seen_ids.iter().any(|x| x == id)
    }
}

pub async fn fetch_rss_items(feed_url: &str, hub: &str) -> Result<Vec<FlixItem>, FlixError> {
    let client = reqwest::Client::new();
    let xml = client.get(feed_url).send().await?.error_for_status()?.text().await?;
    Ok(parse_rss(&xml, hub))
}

pub fn parse_rss(xml: &str, hub: &str) -> Vec<FlixItem> {
    let mut items = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<item") {
        let after = &rest[start..];
        let Some(end_rel) = after.find("</item>") else { break };
        let block = &after[..end_rel + 7];
        let title = inner(block, "title").unwrap_or_else(|| "untitled".into());
        let link = inner(block, "link").unwrap_or_default();
        let guid = inner(block, "guid").unwrap_or_else(|| link.clone());
        if !link.is_empty() {
            items.push(FlixItem {
                id: guid,
                title: strip_cdata(&title),
                link: strip_cdata(&link),
                published: None,
                hub: hub.to_string(),
            });
        }
        rest = &after[end_rel + 7..];
    }
    items
}

fn inner(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let idx = block.find(&open)?;
    let gt = block[idx..].find('>')? + idx + 1;
    let close = format!("</{tag}>");
    let end = block[gt..].find(&close)? + gt;
    Some(block[gt..end].trim().to_string())
}

fn strip_cdata(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("<![CDATA[") {
        if let Some(inner) = rest.strip_suffix("]]>") {
            return inner.trim().to_string();
        }
    }
    t.to_string()
}
