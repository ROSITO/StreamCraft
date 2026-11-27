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
            // Cancelled is sticky: only resume() / request_assemble() leave it,
            // so a late progress update can't revive a cancelled job.
            if job.state == DownloadState::Cancelled {
                return;
            }
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

    pub async fn is_cancelled(&self, id: Uuid) -> bool {
        self.inner
            .lock()
            .await
            .jobs
            .get(&id)
            .is_some_and(|j| j.state == DownloadState::Cancelled)
    }

    /// Resolves once the job is cancelled.
    // ponytail: 250 ms poll, switch to a per-job CancellationToken if latency matters.
    pub async fn cancelled(&self, id: Uuid) {
        while !self.is_cancelled(id).await {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
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

    /// Atomically take the first Queued job (flipped to Downloading under the lock),
    /// so concurrent workers can never run the same job twice.
    pub async fn claim_next(&self) -> Option<Uuid> {
        let mut g = self.inner.lock().await;
        let id = g
            .order
            .iter()
            .copied()
            .find(|id| g.jobs.get(id).is_some_and(|j| j.state == DownloadState::Queued))?;
        let job = g.jobs.get_mut(&id)?;
        job.state = DownloadState::Downloading;
        job.updated_at = Utc::now();
        Some(id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::models::{DetectedMedia, ManifestKind, MediaProtection, MediaVariant};
    use url::Url;

    fn job() -> DownloadJob {
        let url = Url::parse("https://example.com/a.m3u8").unwrap();
        let variant = MediaVariant::new(url.clone());
        let now = Utc::now();
        DownloadJob {
            id: Uuid::new_v4(),
            media: DetectedMedia {
                id: Uuid::new_v4(),
                source_url: url,
                kind: ManifestKind::Hls,
                protection: MediaProtection::None,
                suggested_title: None,
                page_url: None,
                variants: vec![variant.clone()],
                preferred_variant_id: None,
                audio_tracks: vec![],
                subtitle_tracks: vec![],
                is_vod: true,
                segment_count: None,
                duration_seconds: None,
                content_length: None,
                observed_at: now,
            },
            selected_variant: variant,
            selected_audio: None,
            selected_subtitle: None,
            state: DownloadState::Queued,
            destination: None,
            progress: 0.0,
            error_message: None,
            export_metadata: None,
            cookie_header: None,
            referer: None,
            assemble_only: false,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn concurrent_claims_take_a_job_once() {
        let q = DownloadQueue::new();
        let id = q.enqueue(job()).await;
        let (a, b) = tokio::join!(q.claim_next(), q.claim_next());
        assert_eq!([a, b].iter().filter(|c| **c == Some(id)).count(), 1);
        assert!(a.is_none() || b.is_none());
        assert_eq!(q.get(id).await.unwrap().state, DownloadState::Downloading);
    }
}
