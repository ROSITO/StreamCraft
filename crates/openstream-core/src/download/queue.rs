use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::media::models::{DownloadJob, DownloadState};

#[derive(Default)]
pub struct DownloadQueue {
    inner: Mutex<Inner>,
    notify: Notify,
}

#[derive(Default)]
struct Inner {
    jobs: HashMap<Uuid, DownloadJob>,
    order: Vec<Uuid>,
}

impl DownloadQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn enqueue(&self, job: DownloadJob) -> Uuid {
        let id = job.id;
        let mut g = self.inner.lock().await;
        g.order.push(id);
        g.jobs.insert(id, job);
        self.notify.notify_waiters();
        id
    }

    pub async fn update_state(&self, id: Uuid, state: DownloadState, progress: Option<f64>, err: Option<String>) {
        let mut g = self.inner.lock().await;
        if let Some(job) = g.jobs.get_mut(&id) {
            job.state = state;
            if let Some(p) = progress {
                job.progress = p;
            }
            if err.is_some() {
                job.error_message = err;
            }
            job.updated_at = Utc::now();
        }
    }

    pub async fn set_destination(&self, id: Uuid, path: std::path::PathBuf) {
        let mut g = self.inner.lock().await;
        if let Some(job) = g.jobs.get_mut(&id) {
            job.destination = Some(path);
            job.updated_at = Utc::now();
        }
    }

    pub async fn get(&self, id: Uuid) -> Option<DownloadJob> {
        self.inner.lock().await.jobs.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<DownloadJob> {
        let g = self.inner.lock().await;
        g.order
            .iter()
            .filter_map(|id| g.jobs.get(id).cloned())
            .collect()
    }

    pub async fn remove(&self, id: Uuid) -> bool {
        let mut g = self.inner.lock().await;
        let Some(job) = g.jobs.get(&id) else {
            return false;
        };
        match job.state {
            crate::media::models::DownloadState::Queued
            | crate::media::models::DownloadState::Downloading
            | crate::media::models::DownloadState::Assembling => return false,
            _ => {}
        }
        g.jobs.remove(&id);
        g.order.retain(|x| *x != id);
        true
    }

    pub async fn cancel(&self, id: Uuid) -> bool {
        let mut g = self.inner.lock().await;
        if let Some(job) = g.jobs.get_mut(&id) {
            if matches!(
                job.state,
                DownloadState::Queued | DownloadState::Downloading | DownloadState::Assembling
            ) {
                job.state = DownloadState::Cancelled;
                job.updated_at = Utc::now();
                return true;
            }
        }
        false
    }

    /// Reprendre un job cancelled/failed → Queued (segments déjà présents sont sautés).
    pub async fn resume(&self, id: Uuid) -> bool {
        let mut g = self.inner.lock().await;
        if let Some(job) = g.jobs.get_mut(&id) {
            if matches!(
                job.state,
                DownloadState::Cancelled | DownloadState::Failed | DownloadState::Queued
            ) {
                job.state = DownloadState::Queued;
                job.assemble_only = false;
                job.error_message = None;
                job.updated_at = Utc::now();
                drop(g);
                self.notify.notify_waiters();
                return true;
            }
        }
        false
    }

    /// Assembler les segments déjà téléchargés sans re-download.
    pub async fn request_assemble(&self, id: Uuid) -> bool {
        let mut g = self.inner.lock().await;
        if let Some(job) = g.jobs.get_mut(&id) {
            if matches!(
                job.state,
                DownloadState::Cancelled | DownloadState::Failed | DownloadState::Queued
            ) {
                job.assemble_only = true;
                job.state = DownloadState::Queued;
                job.error_message = None;
                job.updated_at = Utc::now();
                drop(g);
                self.notify.notify_waiters();
                return true;
            }
        }
        false
    }

    pub async fn set_credentials(
        &self,
        id: Uuid,
        cookie_header: Option<String>,
        referer: Option<String>,
    ) {
        let mut g = self.inner.lock().await;
        if let Some(job) = g.jobs.get_mut(&id) {
            if cookie_header.is_some() {
                job.cookie_header = cookie_header;
            }
            if referer.is_some() {
                job.referer = referer;
            }
            job.updated_at = Utc::now();
        }
    }

    pub async fn next_queued(&self) -> Option<DownloadJob> {
        let g = self.inner.lock().await;
        g.order
            .iter()
            .filter_map(|id| g.jobs.get(id))
            .find(|j| j.state == DownloadState::Queued)
            .cloned()
    }
}
