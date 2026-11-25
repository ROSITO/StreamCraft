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

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        // Keep last 200 jobs
        let mut clone = self.clone();
        if clone.jobs.len() > 200 {
            clone.jobs.truncate(200);
        }
        std::fs::write(path, serde_json::to_string_pretty(&clone).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?)
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
