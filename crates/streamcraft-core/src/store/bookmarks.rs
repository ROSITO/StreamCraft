use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: Uuid,
    pub title: String,
    pub url: String,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_visited_at: Option<DateTime<Utc>>,
}

impl Bookmark {
    pub fn display_title(&self) -> String {
        let trimmed = self.title.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_else(|| self.url.clone())
    }

    pub fn host_label(&self) -> String {
        Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_else(|| self.url.clone())
    }

    pub fn is_hub(&self) -> bool {
        Url::parse(&self.url)
            .ok()
            .map(|u| {
                let path = u.path();
                path.is_empty() || path == "/"
            })
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BookmarkStore {
    pub items: Vec<Bookmark>,
}

impl BookmarkStore {
    pub fn path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("StreamCraft")
            .join("bookmarks.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap())
    }

    pub fn normalized_key(url: &str) -> String {
        Self::canonical_url(url)
            .unwrap_or_else(|| url.to_string())
            .to_lowercase()
    }

    pub fn canonical_url(url: &str) -> Option<String> {
        let mut u = Url::parse(url).ok()?;
        u.set_fragment(None);
        Some(u.to_string())
    }

    pub fn hub_origin_url(url: &str) -> Option<String> {
        let u = Url::parse(url).ok()?;
        let host = u.host_str()?.to_string();
        let scheme = u.scheme();
        if scheme != "http" && scheme != "https" {
            return None;
        }
        Some(format!("{scheme}://{host}/"))
    }

    pub fn bookmark_matching(&self, url: &str) -> Option<&Bookmark> {
        let key = Self::normalized_key(url);
        self.items
            .iter()
            .find(|b| Self::normalized_key(&b.url) == key)
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmark_matching(url).is_some()
    }

    pub fn add(&mut self, title: String, url: String) -> Result<Bookmark, String> {
        let parsed = Url::parse(&url).map_err(|_| "URL invalide".to_string())?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err("URL invalide".into());
        }
        if let Some(existing) = self.bookmark_matching(&url) {
            return Ok(existing.clone());
        }
        let canonical = Self::canonical_url(&url).unwrap_or(url);
        let resolved = {
            let t = title.trim();
            if t.is_empty() {
                Url::parse(&canonical)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_string()))
                    .unwrap_or_else(|| canonical.clone())
            } else {
                t.to_string()
            }
        };
        let item = Bookmark {
            id: Uuid::new_v4(),
            title: resolved,
            url: canonical,
            created_at: Utc::now(),
            last_visited_at: None,
        };
        self.items.insert(0, item.clone());
        self.save().map_err(|e| e.to_string())?;
        Ok(item)
    }

    pub fn upsert(&mut self, title: String, url: String) {
        let _ = self.add(title, url);
    }

    pub fn remove(&mut self, id: Uuid) -> Result<(), String> {
        self.items.retain(|b| b.id != id);
        self.save().map_err(|e| e.to_string())
    }

    pub fn remove_url(&mut self, url: &str) -> Result<(), String> {
        let key = Self::normalized_key(url);
        self.items.retain(|b| Self::normalized_key(&b.url) != key);
        self.save().map_err(|e| e.to_string())
    }

    /// Returns true if now bookmarked.
    pub fn toggle(&mut self, title: String, url: String) -> Result<bool, String> {
        if let Some(existing) = self.bookmark_matching(&url).map(|b| b.id) {
            self.remove(existing)?;
            return Ok(false);
        }
        self.add(title, url)?;
        Ok(true)
    }

    pub fn rename(&mut self, id: Uuid, title: String) -> Result<(), String> {
        let trimmed = title.trim().to_string();
        if trimmed.is_empty() {
            return Ok(());
        }
        if let Some(b) = self.items.iter_mut().find(|b| b.id == id) {
            b.title = trimmed;
        }
        self.save().map_err(|e| e.to_string())
    }

    pub fn mark_visited(&mut self, id: Uuid) -> Result<(), String> {
        let Some(idx) = self.items.iter().position(|b| b.id == id) else {
            return Ok(());
        };
        self.items[idx].last_visited_at = Some(Utc::now());
        let item = self.items.remove(idx);
        self.items.insert(0, item);
        self.save().map_err(|e| e.to_string())
    }
}
