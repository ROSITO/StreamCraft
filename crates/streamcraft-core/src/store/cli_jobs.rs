//! Persistent CLI job registry for status / wait / cancel across invocations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliJobRecord {
    pub id: Uuid,
    pub title: String,
    pub source_url: String,
    pub state: String,
    pub progress: f64,
    pub destination: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliJobStore {
    pub jobs: Vec<CliJobRecord>,
}

impl CliJobStore {
    pub fn path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("StreamCraft")
            .join("cli_jobs.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::path())
    }

    /// Atomic write (tmp + rename): concurrent readers never see a half-written file.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        // Keep last 200 jobs
        let mut clone = self.clone();
        if clone.jobs.len() > 200 {
            clone.jobs.truncate(200);
        }
        let body = serde_json::to_string_pretty(&clone)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)
    }

    /// Load → mutate → save under an exclusive file lock, so parallel CLI
    /// processes (agents) don't overwrite each other's changes.
    pub fn update<R>(f: impl FnOnce(&mut Self) -> R) -> std::io::Result<R> {
        Self::update_at(&Self::path(), f)
    }

    pub fn update_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> std::io::Result<R> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock()?; // released when `lock` drops
        let mut store = Self::load_from(path);
        let out = f(&mut store);
        store.save_to(path)?;
        Ok(out)
    }

    pub fn upsert(&mut self, job: CliJobRecord) {
        if let Some(existing) = self.jobs.iter_mut().find(|j| j.id == job.id) {
            *existing = job;
        } else {
            self.jobs.insert(0, job);
        }
    }

    pub fn get(&self, id: Uuid) -> Option<&CliJobRecord> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut CliJobRecord> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn request_cancel(&mut self, id: Uuid) -> bool {
        if let Some(j) = self.get_mut(id) {
            if matches!(j.state.as_str(), "queued" | "downloading" | "assembling") {
                j.cancel_requested = true;
                j.state = "cancelled".into();
                j.updated_at = Utc::now();
                return true;
            }
        }
        false
    }

    pub fn is_cancel_requested(id: Uuid) -> bool {
        Self::load()
            .get(id)
            .map(|j| j.cancel_requested || j.state == "cancelled")
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: Uuid) -> CliJobRecord {
        let now = Utc::now();
        CliJobRecord {
            id,
            title: "t".into(),
            source_url: "https://example.com/a.m3u8".into(),
            state: "downloading".into(),
            progress: 0.0,
            destination: None,
            error_message: None,
            created_at: now,
            updated_at: now,
            cancel_requested: false,
        }
    }

    #[test]
    fn parallel_updates_are_not_lost() {
        let dir = std::env::temp_dir().join(format!("sc-clijobs-{}", Uuid::new_v4()));
        let path = dir.join("cli_jobs.json");
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    CliJobStore::update_at(&path, |s| s.upsert(record(Uuid::new_v4()))).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(CliJobStore::load_from(&path).jobs.len(), 16);
        let _ = std::fs::remove_dir_all(dir);
    }
}
