use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::stream::{self, StreamExt};
use tokio::io::AsyncWriteExt;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::assemble::{av_sync, remux};
use crate::download::normalize::normalize_segment_file;
use crate::download::planner::{cookie_allowed, plan_for_variant, prefix_units, DownloadUnit};
use crate::download::queue::DownloadQueue;
use url::Url;

use crate::media::cdn_gate::{apply_gate_token, best_gate_token_for, has_gate_token};
use crate::media::export_naming::{ExportMetadata, ExportNaming};
use crate::media::models::{DetectedMedia, DownloadJob, DownloadState, ManifestKind, MediaProtection, MediaTrack, MediaVariant};
use crate::store::history::{DownloadHistoryStore, HistoryEntry};
use crate::store::prefs::AppPreferences;
use streamcraft_ffmpeg::FFmpegPaths;

/// Browser session replayed for a job. Given at enqueue time so a running worker
/// can never claim the job before its cookies are attached.
#[derive(Debug, Clone, Default)]
pub struct JobCredentials {
    pub cookie_header: Option<String>,
    pub referer: Option<String>,
}

pub struct DownloadService {
    pub queue: Arc<DownloadQueue>,
    pub client: reqwest::Client,
    pub prefs: AppPreferences,
    /// Set when the configured proxy is invalid: jobs are refused rather than going direct.
    proxy_error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum DownloadError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("server ignored the byte range request (HTTP {0})")]
    RangeIgnored(u16),
}

impl DownloadError {
    fn is_transient(&self) -> bool {
        match self {
            Self::Http(e) => {
                e.is_timeout()
                    || e.is_connect()
                    || e.is_request()
                    || e.is_body()
                    // A connection dropped mid-body surfaces as a decode error from `chunk()`.
                    || e.is_decode()
                    || e.status().is_some_and(|s| s.as_u16() == 429 || s.is_server_error())
            }
            Self::Io(_) | Self::RangeIgnored(_) => false,
        }
    }
}

impl DownloadService {
    pub fn new(prefs: AppPreferences) -> Self {
        let mut builder = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
            // Idle timeout, not total: a long progressive MP4 must not be cut at N seconds.
            .read_timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(8);
        let mut proxy_error = None;
        if let Some(proxy) = prefs.explicit_proxy_url() {
            match reqwest::Proxy::all(&proxy) {
                Ok(p) => builder = builder.proxy(p),
                Err(e) => {
                    error!(error = %e, "invalid proxy setting: downloads refused");
                    proxy_error = Some(format!("invalid proxy setting ({e}): download refused"));
                }
            }
        }
        Self {
            queue: DownloadQueue::new(),
            client: builder.build().expect("client"),
            prefs,
            proxy_error,
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
        credentials: JobCredentials,
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
        let referer = credentials.referer.or_else(|| {
            Some(
                media
                    .page_url
                    .as_ref()
                    .map_or(&variant.playlist_url, |u| u)
                    .to_string(),
            )
        });
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
            cookie_header: credentials.cookie_header,
            referer,
            assemble_only: false,
            created_at: now,
            updated_at: now,
        };
        Ok(self.queue.enqueue(job).await)
    }

    /// Runs the job; any error leaves it `Failed` (never stuck in Downloading/Assembling),
    /// so the UI can still resume or remove it.
    pub async fn run_job(&self, job_id: Uuid) -> Result<PathBuf, String> {
        let result = self.run_job_inner(job_id).await;
        if let Err(e) = &result
            && let Some(job) = self.queue.get(job_id).await
            && !matches!(job.state, DownloadState::Cancelled | DownloadState::Failed)
        {
            self.queue
                .update_state(job_id, DownloadState::Failed, None, Some(e.clone()))
                .await;
        }
        result
    }

    async fn run_job_inner(&self, job_id: Uuid) -> Result<PathBuf, String> {
        let job = self
            .queue
            .get(job_id)
            .await
            .ok_or_else(|| "job not found".to_string())?;
        if let Some(e) = &self.proxy_error {
            return Err(e.clone());
        }
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

        let source = job.media.source_url.clone();
        let (variant, audio_opt) = unlock_hls_urls(&job);
        let video_plan = plan_for_variant(
            &self.client,
            &variant,
            job.media.kind,
            &source,
            &referer,
            cookie.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())?;

        if video_plan.units.is_empty() {
            return Err("empty download plan".into());
        }

        let audio_plan = if let Some(audio) = audio_opt.as_ref() {
            let audio_variant = MediaVariant::new(audio.url.clone());
            match plan_for_variant(
                &self.client,
                &audio_variant,
                ManifestKind::Hls,
                &source,
                &referer,
                cookie.as_deref(),
            )
            .await
            {
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
            // max_retries = retries after the first attempt (0 → exactly one try).
            let attempts = self.prefs.max_retries.min(8) as u32 + 1;
            let client = self.client.clone();
            let referer_c = referer.clone();
            let cookie_c = cookie.clone();

            let queue = self.queue.clone();
            let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let failed = std::sync::Arc::new(tokio::sync::Mutex::new(None::<(usize, String)>));

            let downloads = stream::iter(plan.units.iter().cloned())
                .for_each_concurrent(concurrency, |unit| {
                    let client = client.clone();
                    let parts_dir = parts_dir.clone();
                    let referer = referer_c.clone();
                    let cookie = cookie_c.clone();
                    let queue = queue.clone();
                    let done = done.clone();
                    let failed = failed.clone();
                    let source = source.clone();
                    async move {
                        if failed.lock().await.is_some() {
                            return;
                        }
                        let dest = parts_dir.join(&unit.file_name);
                        let r = download_unit_with_retry(
                            &client,
                            &unit,
                            &dest,
                            &referer,
                            cookie.as_deref(),
                            &source,
                            attempts,
                        )
                        .await;
                        if let Err(e) = r {
                            *failed.lock().await = Some((unit.index, e.to_string()));
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
                });
            // Dropping `downloads` aborts in-flight requests; `.part` files are
            // overwritten on resume, completed segments are kept.
            tokio::select! {
                _ = downloads => {}
                _ = self.queue.cancelled(job_id) => return Err("cancelled".into()),
            }

            if let Some((idx, e)) = failed.lock().await.clone() {
                self.queue
                    .update_state(job_id, DownloadState::Failed, None, Some(e.clone()))
                    .await;
                return Err(format!("segment {idx}: {e}"));
            }
        }

        if self.queue.is_cancelled(job_id).await {
            return Err("cancelled".into());
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

        // Cancelled while assembling: don't deliver a file the user cancelled.
        // Segments stay in .parts so "Assemble" / resume still work.
        if self.queue.is_cancelled(job_id).await {
            let _ = tokio::fs::remove_file(&out_path).await;
            return Err("cancelled".into());
        }

        // Optional subtitle sidecar. An HLS subtitle rendition is a playlist of
        // WebVTT segments: fetch them and join into one .vtt (Swift exportSubtitle).
        if let Some(sub) = job.selected_subtitle.as_ref() {
            let sidecar =
                fetch_subtitle(&self.client, &sub.url, &referer, cookie.as_deref(), &source).await;
            match sidecar {
                Ok(Some((text, ext))) => {
                    let _ = tokio::fs::write(out_path.with_extension(ext), text).await;
                }
                Ok(None) => warn!("subtitle playlist unreadable — skipped"),
                Err(e) => warn!(error = %e, "subtitle download failed — skipped"),
            }
        }

        let _ = tokio::fs::remove_dir_all(&parts_dir).await;
        // Drop `.parts/` too once empty (fails harmlessly while other jobs use it).
        if let Some(root) = parts_dir.parent() {
            let _ = tokio::fs::remove_dir(root).await;
        }

        self.queue.set_destination(job_id, out_path.clone()).await;
        self.queue
            .update_state(job_id, DownloadState::Completed, Some(1.0), None)
            .await;

        let entry = HistoryEntry {
            id: job_id,
            title: job.media.display_title(),
            source_url: job.media.source_url.to_string(),
            output_path: out_path.display().to_string(),
            completed_at: Utc::now(),
        };
        let _ = tokio::task::spawn_blocking(move || {
            let mut hist = DownloadHistoryStore::load();
            hist.push(entry);
            hist.save()
        })
        .await;

        info!(%job_id, path = %out_path.display(), "download completed");
        Ok(out_path)
    }
}


/// Subtitle text and its extension. An HLS subtitle rendition is a playlist of
/// WebVTT segments: fetch them and join into one .vtt (Swift exportSubtitle).
/// `Ok(None)` when the playlist can't be parsed.
async fn fetch_subtitle(
    client: &reqwest::Client,
    url: &Url,
    referer: &str,
    cookie: Option<&str>,
    source: &Url,
) -> Result<Option<(String, String)>, reqwest::Error> {
    let get = |u: Url| {
        let mut req = client.get(u.clone()).header("Referer", referer);
        if let Some(c) = cookie.filter(|_| cookie_allowed(&u, source)) {
            req = req.header("Cookie", c);
        }
        async move { req.send().await?.error_for_status()?.text().await }
    };
    let body = get(url.clone()).await?;
    if !body.trim_start().starts_with("#EXTM3U") {
        let ext = url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .and_then(|n| n.rsplit_once('.'))
            .map(|(_, e)| e.to_lowercase())
            .unwrap_or_else(|| "vtt".into());
        return Ok(Some((body, ext)));
    }
    let Ok(playlist) = crate::media::hls::HLSParser::parse(&body, url) else {
        return Ok(None);
    };
    let mut parts = Vec::new();
    for seg in playlist.media.map(|m| m.segments).unwrap_or_default() {
        parts.push(get(seg.uri).await?);
    }
    Ok(Some((join_webvtt(&parts), "vtt".to_string())))
}

/// Join WebVTT segments into one file: keep the first header, drop the
/// `WEBVTT` header block of every following segment.
fn join_webvtt(parts: &[String]) -> String {
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let part = part.trim_start_matches('\u{feff}');
        let body = if i > 0 && part.trim_start().starts_with("WEBVTT") {
            // Header runs up to the first blank line.
            part.split_once("\n\n").map_or("", |(_, rest)| rest)
        } else {
            part
        };
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
        }
        out.push_str(body.trim_end());
        out.push('\n');
    }
    out
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
    unit: &DownloadUnit,
    dest: &Path,
    referer: &str,
    cookie: Option<&str>,
    source: &Url,
    attempts: u32,
) -> Result<(), DownloadError> {
    let mut i = 0;
    loop {
        match download_unit(client, unit, dest, referer, cookie, source).await {
            Ok(()) => return Ok(()),
            Err(e) if e.is_transient() && i + 1 < attempts => {
                let wait = 1u64 << i.min(4);
                warn!(%wait, error = %e, "segment retry");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                i += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn download_unit(
    client: &reqwest::Client,
    unit: &DownloadUnit,
    dest: &Path,
    referer: &str,
    cookie: Option<&str>,
    source: &Url,
) -> Result<(), DownloadError> {
    // Only complete files ever exist under `dest` (written to `.part`, then renamed).
    if let Ok(meta) = tokio::fs::metadata(dest).await {
        if meta.len() > 0 {
            return Ok(());
        }
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut req = client
        .get(unit.url.clone())
        .header("Referer", referer)
        .header("Origin", origin_hint(referer));
    if let Some(c) = cookie.filter(|c| !c.is_empty() && cookie_allowed(&unit.url, source)) {
        req = req.header("Cookie", c);
    }
    if let Some((start, end)) = unit.byte_range {
        req = req.header("Range", format!("bytes={start}-{end}"));
    }
    let mut resp = req.send().await?.error_for_status()?;
    // A 200 would be the whole file, not the sub-range.
    if unit.byte_range.is_some() && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(DownloadError::RangeIgnored(resp.status().as_u16()));
    }
    let part = part_path(dest);
    let mut file = tokio::fs::File::create(&part).await?;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&part, dest).await?;
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::models::MediaProtection;

    /// Real HTTP: an HLS subtitle playlist becomes one .vtt, not the playlist text.
    #[tokio::test]
    async fn hls_subtitle_playlist_becomes_vtt() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.starts_with("GET /subs.m3u8") {
                    "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\nsub0.vtt\n#EXTINF:6,\nsub1.vtt\n#EXT-X-ENDLIST\n"
                } else if req.starts_with("GET /sub0.vtt") {
                    "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nBonjour\n"
                } else {
                    "WEBVTT\n\n00:00:07.000 --> 00:00:08.000\nSalut\n"
                };
                let _ = sock
                    .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
                    .await;
            }
        });
        let url = Url::parse(&format!("http://{addr}/subs.m3u8")).unwrap();
        let (text, ext) = fetch_subtitle(&reqwest::Client::new(), &url, "", None, &url)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ext, "vtt");
        assert!(!text.contains("#EXTM3U"));
        assert_eq!(text.matches("WEBVTT").count(), 1);
        assert!(text.contains("Bonjour") && text.contains("Salut"));
    }

    #[test]
    fn webvtt_segments_join_with_one_header() {
        let a = "WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:900000,LOCAL:00:00:00.000\n\n00:00:01.000 --> 00:00:02.000\nBonjour\n".to_string();
        let b = "WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:900000,LOCAL:00:00:00.000\n\n00:00:07.000 --> 00:00:08.000\nSalut\n".to_string();
        let joined = join_webvtt(&[a, b]);
        assert_eq!(joined.matches("WEBVTT").count(), 1);
        assert!(joined.contains("Bonjour\n\n00:00:07.000 --> 00:00:08.000\nSalut\n"));
    }

    #[test]
    fn part_path_keeps_segment_name() {
        assert_eq!(
            part_path(Path::new("/p/00001_seg.ts")),
            Path::new("/p/00001_seg.ts.part")
        );
    }

    fn test_media(url: Url, kind: ManifestKind) -> DetectedMedia {
        DetectedMedia {
            id: Uuid::new_v4(),
            source_url: url.clone(),
            kind,
            protection: MediaProtection::None,
            suggested_title: None,
            page_url: None,
            variants: vec![MediaVariant::new(url)],
            preferred_variant_id: None,
            audio_tracks: vec![],
            subtitle_tracks: vec![],
            is_vod: true,
            segment_count: None,
            duration_seconds: None,
            content_length: None,
            observed_at: Utc::now(),
        }
    }

    fn test_service() -> DownloadService {
        DownloadService::new(AppPreferences {
            export_dir: std::env::temp_dir().join(format!("sc-svc-{}", Uuid::new_v4())),
            ..AppPreferences::default()
        })
    }

    /// A job that errors before assembling must end `Failed`, not stuck `Downloading`.
    #[tokio::test]
    async fn failing_job_is_marked_failed() {
        let svc = test_service();
        // MPD fetch fails (unresolvable host) → error after Downloading
        let url = Url::parse("https://example.invalid/manifest.mpd").unwrap();
        let media = test_media(url, ManifestKind::Dash);
        let id = svc.enqueue_media(media, None, None, None, None, JobCredentials::default()).await.unwrap();
        assert!(svc.run_job(id).await.is_err());
        let job = svc.queue.get(id).await.unwrap();
        assert_eq!(job.state, DownloadState::Failed);
        assert!(svc.queue.resume(id).await, "failed job must be resumable");
    }

    #[tokio::test]
    async fn progress_update_does_not_revive_cancelled_job() {
        let svc = test_service();
        let url = Url::parse("https://example.invalid/a.mp4").unwrap();
        let id = svc
            .enqueue_media(test_media(url, ManifestKind::Progressive), None, None, None, None, JobCredentials::default())
            .await
            .unwrap();
        let q = &svc.queue;
        q.update_state(id, DownloadState::Downloading, Some(0.1), None).await;
        assert!(q.cancel(id).await);
        q.update_state(id, DownloadState::Downloading, Some(0.5), None).await;
        assert!(q.is_cancelled(id).await);
        assert!(q.resume(id).await);
        assert_eq!(q.get(id).await.unwrap().state, DownloadState::Queued);
    }

    /// Cancel must abort a stalled transfer (here the read timeout is 60 s).
    #[tokio::test]
    async fn cancel_interrupts_running_download() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000000\r\n\r\nxx")
                .await;
            tokio::time::sleep(Duration::from_secs(120)).await; // stall
        });
        let svc = Arc::new(test_service());
        let url = Url::parse(&format!("http://{addr}/movie.mp4")).unwrap();
        let id = svc
            .enqueue_media(test_media(url, ManifestKind::Progressive), None, None, None, None, JobCredentials::default())
            .await
            .unwrap();
        let run = tokio::spawn({
            let svc = svc.clone();
            async move { svc.run_job(id).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(svc.queue.cancel(id).await);
        let r = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("cancel did not stop the download")
            .unwrap();
        assert_eq!(r, Err("cancelled".into()));
        assert_eq!(svc.queue.get(id).await.unwrap().state, DownloadState::Cancelled);
    }
}
