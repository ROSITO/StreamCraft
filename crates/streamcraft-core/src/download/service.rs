use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::stream::{self, StreamExt};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};
use uuid::Uuid;

use crate::assemble::{av_sync, remux};
use crate::download::normalize::normalize_segment_file;
use crate::download::planner::{plan_for_variant, prefix_units};
use crate::download::queue::DownloadQueue;
use url::Url;

use crate::media::cdn_gate::{apply_gate_token, best_gate_token_for, has_gate_token};
use crate::media::export_naming::{ExportMetadata, ExportNaming};
use crate::media::models::{DetectedMedia, DownloadJob, DownloadState, ManifestKind, MediaProtection, MediaTrack, MediaVariant};
use crate::store::history::{DownloadHistoryStore, HistoryEntry};
use crate::store::prefs::AppPreferences;
use streamcraft_ffmpeg::FFmpegPaths;

pub struct DownloadService {
    pub queue: Arc<DownloadQueue>,
    pub client: reqwest::Client,
    pub prefs: AppPreferences,
}

impl DownloadService {
    pub fn new(prefs: AppPreferences) -> Self {
        let mut builder = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(8);
        if let Some(proxy) = prefs.explicit_proxy_url() {
            if let Ok(p) = reqwest::Proxy::all(&proxy) {
                builder = builder.proxy(p);
            }
        }
        Self {
            queue: DownloadQueue::new(),
            client: builder.build().expect("client"),
            prefs,
        }
    }

    pub fn with_queue(prefs: AppPreferences, queue: Arc<DownloadQueue>) -> Self {
        let mut s = Self::new(prefs);
        s.queue = queue;
        s
    }

    pub async fn enqueue_media(
        &self,
        media: DetectedMedia,
        metadata: Option<ExportMetadata>,
        variant_id: Option<Uuid>,
        audio_track_id: Option<Uuid>,
        subtitle_track_id: Option<Uuid>,
    ) -> Result<Uuid, String> {
        if let MediaProtection::Drm { reason } = &media.protection {
            return Err(format!("DRM protected: {reason}"));
        }
        let variant = if let Some(id) = variant_id {
            media
                .variants
                .iter()
                .find(|v| v.id == id)
                .cloned()
                .or_else(|| media.preferred_variant().cloned())
        } else {
            media.preferred_variant().cloned()
        }
        .ok_or_else(|| "no variant".to_string())?;

        let selected_audio = resolve_audio_track(&media, audio_track_id);
        let selected_subtitle = subtitle_track_id.and_then(|id| {
            media.subtitle_tracks.iter().find(|t| t.id == id).cloned()
        });

        let now = Utc::now();
        let referer = media
            .page_url
            .as_ref()
            .map(|u| u.as_str().to_string())
            .or_else(|| Some(variant.playlist_url.as_str().to_string()));
        let job = DownloadJob {
            id: Uuid::new_v4(),
            media,
            selected_variant: variant,
            selected_audio,
            selected_subtitle,
            state: DownloadState::Queued,
            destination: None,
            progress: 0.0,
            error_message: None,
            export_metadata: metadata,
            cookie_header: None,
            referer,
            assemble_only: false,
            created_at: now,
            updated_at: now,
        };
        Ok(self.queue.enqueue(job).await)
    }

    pub async fn set_job_credentials(
        &self,
        job_id: Uuid,
        cookie_header: Option<String>,
        referer: Option<String>,
    ) {
        self.queue
            .set_credentials(job_id, cookie_header, referer)
            .await;
    }

    pub async fn run_job(&self, job_id: Uuid) -> Result<PathBuf, String> {
        let job = self
            .queue
            .get(job_id)
            .await
            .ok_or_else(|| "job not found".to_string())?;
        if matches!(job.media.protection, MediaProtection::Drm { .. }) {
            self.queue
                .update_state(job_id, DownloadState::Failed, None, Some("DRM refused".into()))
                .await;
            return Err("DRM refused".into());
        }

        self.queue
            .update_state(job_id, DownloadState::Downloading, Some(0.0), None)
            .await;

        let referer = job
            .referer
            .clone()
            .or_else(|| {
                job.media
                    .page_url
                    .as_ref()
                    .map(|u| u.as_str().to_string())
            })
            .unwrap_or_else(|| job.selected_variant.playlist_url.as_str().to_string());
        let cookie = job.cookie_header.clone();
        let assemble_only = job.assemble_only;

        let (variant, audio_opt) = unlock_hls_urls(&job);
        let video_plan = plan_for_variant(&self.client, &variant, job.media.kind)
            .await
            .map_err(|e| e.to_string())?;

        if video_plan.units.is_empty() {
            return Err("empty download plan".into());
        }

        let audio_plan = if let Some(audio) = audio_opt.as_ref() {
            let audio_variant = MediaVariant::new(audio.url.clone());
            match plan_for_variant(&self.client, &audio_variant, ManifestKind::Hls).await {
                Ok(p) if !p.units.is_empty() => Some(prefix_units(p, "audio/")),
                Ok(_) => None,
                Err(e) => {
                    warn!(error = %e, "audio playlist plan failed — video-only");
                    None
                }
            }
        } else {
            None
        };

        let mut plan = video_plan.clone();
        if let Some(ap) = audio_plan.as_ref() {
            let offset = plan.units.len();
            for (i, mut u) in ap.units.clone().into_iter().enumerate() {
                u.index = offset + i;
                plan.units.push(u);
            }
        }

        let parts_dir = self
            .prefs
            .export_dir
            .join(".parts")
            .join(job_id.to_string());
        tokio::fs::create_dir_all(&parts_dir)
            .await
            .map_err(|e| e.to_string())?;
        if audio_plan.is_some() {
            tokio::fs::create_dir_all(parts_dir.join("audio"))
                .await
                .map_err(|e| e.to_string())?;
        }

        let total = plan.units.len().max(1) as f64;
        if !assemble_only {
            let concurrency = self.prefs.max_concurrency.clamp(1, 32);
            let retries = self.prefs.max_retries.clamp(0, 8);
            let client = self.client.clone();
            let referer_c = referer.clone();
            let cookie_c = cookie.clone();

            let queue = self.queue.clone();
            let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let failed = std::sync::Arc::new(tokio::sync::Mutex::new(None::<(usize, String)>));

            stream::iter(plan.units.iter().cloned())
                .for_each_concurrent(concurrency, |unit| {
                    let client = client.clone();
                    let parts_dir = parts_dir.clone();
                    let referer = referer_c.clone();
                    let cookie = cookie_c.clone();
                    let queue = queue.clone();
                    let done = done.clone();
                    let failed = failed.clone();
                    let retries = retries;
                    async move {
                        if failed.lock().await.is_some() {
                            return;
                        }
                        let dest = parts_dir.join(&unit.file_name);
                        let r = download_unit_with_retry(
                            &client,
                            &unit.url,
                            &dest,
                            &referer,
                            cookie.as_deref(),
                            retries as u32,
                        )
                        .await;
                        if let Err(e) = r {
                            *failed.lock().await = Some((unit.index, e));
                            return;
                        }
                        if !unit.is_init {
                            let _ = normalize_segment_file(&dest).await;
                        }
                        let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        queue
                            .update_state(
                                job_id,
                                DownloadState::Downloading,
                                Some((n as f64 / total) * 0.95),
                                None,
                            )
                            .await;
                    }
                })
                .await;

            if let Some((idx, e)) = failed.lock().await.clone() {
                self.queue
                    .update_state(job_id, DownloadState::Failed, None, Some(e.clone()))
                    .await;
                return Err(format!("segment {idx}: {e}"));
            }
        }

        // Cancel check
        if let Some(j) = self.queue.get(job_id).await {
            if j.state == DownloadState::Cancelled {
                return Err("cancelled".into());
            }
        }

        self.queue
            .update_state(job_id, DownloadState::Assembling, Some(0.97), None)
            .await;

        let kind = match job.media.kind {
            ManifestKind::Hls => "hls",
            ManifestKind::Dash => "dash",
            ManifestKind::Progressive => "mp4",
        };
        let meta = job.export_metadata.clone().unwrap_or_else(|| {
            let seed = job.media.display_title();
            let (title, year) = ExportNaming::parse_title_and_year(&seed);
            ExportMetadata {
                title,
                year,
                show: None,
                season: None,
                episode: None,
                episode_title: None,
                naming_preset: Some(self.prefs.naming_preset),
            }
        });
        let preset = meta
            .naming_preset
            .unwrap_or(self.prefs.naming_preset);
        let ctx = meta.into_context(kind, "mp4");
        let relative = ExportNaming::relative_path(preset.template(), &ctx);
        let dest_dir = self.prefs.export_dir.clone();
        let file_name = Path::new(&relative)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("export.mp4");
        let parent = Path::new(&relative).parent().unwrap_or(Path::new(""));
        let out_dir = dest_dir.join(parent);
        let out_path = ExportNaming::unique_path(&out_dir, file_name);

        let ffmpeg = FFmpegPaths::resolve().map_err(|e| e.to_string())?;
        if let Some(ap) = audio_plan.as_ref() {
            remux::assemble_demuxed_hls(&ffmpeg, &video_plan, ap, &parts_dir, &out_path)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            remux::assemble(&ffmpeg, &video_plan, &parts_dir, &out_path)
                .await
                .map_err(|e| e.to_string())?;
        }

        // Swift: post-mux AV duration check + realign like player when A≠V
        match av_sync::post_export_fixup(&ffmpeg, &out_path).await {
            Ok(Some(r)) => info!(msg = %r.user_message(), "AV sync fixup"),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "AV sync fixup warning"),
        }

        // Optional subtitle sidecar (download raw if URL is vtt/srt)
        if let Some(sub) = job.selected_subtitle.as_ref() {
            let ext = sub
                .url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .and_then(|n| n.rsplit_once('.'))
                .map(|(_, e)| e.to_lowercase())
                .unwrap_or_else(|| "vtt".into());
            let side = out_path.with_extension(&ext);
            let mut sub_req = self
                .client
                .get(sub.url.clone())
                .header("Referer", &referer);
            if let Some(c) = cookie.as_ref() {
                sub_req = sub_req.header("Cookie", c);
            }
            if let Ok(bytes) = sub_req
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                if let Ok(body) = bytes.bytes().await {
                    let _ = tokio::fs::write(&side, body).await;
                }
            }
        }

        let _ = tokio::fs::remove_dir_all(&parts_dir).await;

        self.queue.set_destination(job_id, out_path.clone()).await;
        self.queue
            .update_state(job_id, DownloadState::Completed, Some(1.0), None)
            .await;

        let mut hist = DownloadHistoryStore::load();
        hist.push(HistoryEntry {
            id: job_id,
            title: job.media.display_title(),
            source_url: job.media.source_url.to_string(),
            output_path: out_path.display().to_string(),
            completed_at: Utc::now(),
        });
        let _ = hist.save();

        info!(%job_id, path = %out_path.display(), "download completed");
        Ok(out_path)
    }
}


fn resolve_audio_track(media: &DetectedMedia, audio_track_id: Option<Uuid>) -> Option<MediaTrack> {
    if let Some(id) = audio_track_id {
        if let Some(t) = media.audio_tracks.iter().find(|t| t.id == id).cloned() {
            return Some(t);
        }
    }
    // Swift defaultAudioTrack: FR > DEFAULT > first, for HLS/DASH with external tracks
    if media.audio_tracks.is_empty() {
        return None;
    }
    match media.kind {
        ManifestKind::Hls | ManifestKind::Dash => {
            MediaTrack::preferred_audio(&media.audio_tracks).cloned()
        }
        ManifestKind::Progressive => None,
    }
}

async fn download_unit_with_retry(
    client: &reqwest::Client,
    url: &url::Url,
    dest: &Path,
    referer: &str,
    cookie: Option<&str>,
    attempts: u32,
) -> Result<(), String> {
    let mut last = String::new();
    for i in 0..attempts {
        match download_unit(client, url, dest, referer, cookie).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e;
                let transient = last.contains("429")
                    || last.contains("502")
                    || last.contains("503")
                    || last.contains("504")
                    || last.contains("timed out")
                    || last.contains("error sending request");
                if !transient || i + 1 == attempts {
                    break;
                }
                let wait = 1u64 << i.min(4);
                warn!(%wait, error = %last, "segment retry");
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        }
    }
    Err(last)
}

async fn download_unit(
    client: &reqwest::Client,
    url: &url::Url,
    dest: &Path,
    referer: &str,
    cookie: Option<&str>,
) -> Result<(), String> {
    if let Ok(meta) = tokio::fs::metadata(dest).await {
        if meta.len() > 0 {
            return Ok(());
        }
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut req = client
        .get(url.clone())
        .header("Referer", referer)
        .header("Origin", origin_hint(referer));
    if let Some(c) = cookie {
        if !c.is_empty() {
            req = req.header("Cookie", c);
        }
    }
    let mut resp = req
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| e.to_string())?;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

fn origin_hint(referer: &str) -> String {
    Url::parse(referer)
        .ok()
        .map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")))
        .unwrap_or_else(|| referer.to_string())
}

/// Propagate browser-captured `gk` onto variant / audio playlist URLs before planning.
fn unlock_hls_urls(job: &DownloadJob) -> (MediaVariant, Option<MediaTrack>) {
    let anchor = &job.selected_variant.playlist_url;
    let mut urls = vec![&job.selected_variant.playlist_url, &job.media.source_url];
    for v in &job.media.variants {
        urls.push(&v.playlist_url);
    }
    for t in &job.media.audio_tracks {
        urls.push(&t.url);
    }
    if let Some(a) = job.selected_audio.as_ref() {
        urls.push(&a.url);
    }
    let Some(tok) = best_gate_token_for(Some(anchor), urls) else {
        return (job.selected_variant.clone(), job.selected_audio.clone());
    };
    let mut variant = job.selected_variant.clone();
    if !has_gate_token(&variant.playlist_url) {
        variant.playlist_url = apply_gate_token(&variant.playlist_url, &tok);
    }
    let audio = job.selected_audio.as_ref().map(|a| {
        let mut t = a.clone();
        if !has_gate_token(&t.url) {
            t.url = apply_gate_token(&t.url, &tok);
        }
        t
    });
    (variant, audio)
}
