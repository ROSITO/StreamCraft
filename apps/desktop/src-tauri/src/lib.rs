#[cfg(target_os = "macos")]
mod macos_media_hook;
mod vpn;

use streamcraft_core::agent::popup_guard::PopupGuard;
use streamcraft_core::download::queue::DownloadQueue;
use streamcraft_core::download::service::DownloadService;
use streamcraft_core::media::dash::DASHParser;
use streamcraft_core::media::detector::MediaDetector;
use streamcraft_core::media::heuristics::MediaURLHeuristics;
use streamcraft_core::media::hls::HLSParser;
use streamcraft_core::media::models::{
    DetectedMedia, DownloadJob, MediaClassification, MediaVariant, NetworkMediaCandidate,
    Resolution,
};
use streamcraft_core::store::history::DownloadHistoryStore;
use streamcraft_core::store::prefs::AppPreferences;
use streamcraft_ffmpeg::FFmpegPaths;
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, WebviewUrl};
use url::Url;
use uuid::Uuid;

struct AppState {
    detected: std::sync::Mutex<Vec<DetectedMedia>>,
    seen_urls: std::sync::Mutex<HashSet<String>>,
    prefs: std::sync::Mutex<AppPreferences>,
    /// Shared queue; downloads run one-at-a-time from the spawn drain loop.
    queue: Arc<DownloadQueue>,
    page_title: std::sync::Mutex<Option<String>>,
    page_url: std::sync::Mutex<Option<String>>,
    /// Payloads from WKScriptMessageHandler (all frames) — clone Swift streamcraftMedia.
    pending_media: std::sync::Arc<std::sync::Mutex<Vec<ScriptMediaPayload>>>,
    raw_candidates: std::sync::Mutex<Vec<NetworkMediaCandidate>>,
    cookie_snapshot: std::sync::Mutex<streamcraft_core::session::SessionCredentialSnapshot>,
    auto_enqueued_keys: std::sync::Mutex<HashSet<String>>,
    pending_batch_pages: std::sync::Mutex<Vec<String>>,
}

#[derive(Serialize)]
struct FFmpegStatus {
    ok: bool,
    source: Option<String>,
    path: Option<String>,
}

#[derive(Serialize)]
struct DownloadStartResult {
    job_id: String,
    message: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct ScriptMediaPayload {
    pub url: String,
    pub mime: Option<String>,
    pub source: Option<String>,
    pub page: Option<String>,
}

#[tauri::command]
fn get_preferences(state: tauri::State<'_, AppState>) -> AppPreferences {
    state.prefs.lock().unwrap().clone()
}

#[tauri::command]
fn save_preferences(prefs: AppPreferences, state: tauri::State<'_, AppState>) -> Result<(), String> {
    prefs.save().map_err(|e| e.to_string())?;
    *state.prefs.lock().unwrap() = prefs;
    Ok(())
}

#[tauri::command]
fn ffmpeg_status() -> FFmpegStatus {
    match FFmpegPaths::resolve() {
        Ok(p) => FFmpegStatus {
            ok: true,
            source: Some(p.source),
            path: Some(p.ffmpeg.display().to_string()),
        },
        Err(_) => FFmpegStatus {
            ok: false,
            source: None,
            path: None,
        },
    }
}

#[tauri::command]
fn list_detected(state: tauri::State<'_, AppState>) -> Vec<DetectedMedia> {
    state.detected.lock().unwrap().clone()
}

#[tauri::command]
fn clear_detected(state: tauri::State<'_, AppState>) {
    state.detected.lock().unwrap().clear();
    state.seen_urls.lock().unwrap().clear();
    state.raw_candidates.lock().unwrap().clear();
    state.auto_enqueued_keys.lock().unwrap().clear();
}

#[tauri::command]
async fn list_jobs(state: tauri::State<'_, AppState>) -> Result<Vec<DownloadJob>, String> {
    Ok(state.queue.list().await)
}

#[tauri::command]
async fn cancel_job(job_id: String, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let id = Uuid::parse_str(&job_id).map_err(|e| e.to_string())?;
    Ok(state.queue.cancel(id).await)
}

#[tauri::command]
async fn remove_job(job_id: String, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let id = Uuid::parse_str(&job_id).map_err(|e| e.to_string())?;
    Ok(state.queue.remove(id).await)
}

#[tauri::command]
fn list_history() -> Vec<streamcraft_core::store::history::HistoryEntry> {
    DownloadHistoryStore::load().entries
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if p.is_dir() {
        open::that(&p).map_err(|e| e.to_string())
    } else if let Some(parent) = p.parent() {
        open::that(parent).map_err(|e| e.to_string())
    } else {
        open::that(&p).map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn reveal_exports(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let dir = state.prefs.lock().unwrap().export_dir.clone();
    let _ = std::fs::create_dir_all(&dir);
    open::that(dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn vpn_status(state: tauri::State<'_, AppState>) -> vpn::SystemVpnStatus {
    let filter = state.prefs.lock().unwrap().vpn_indicator_filter;
    vpn::refresh(filter)
}

#[tauri::command]
fn pick_export_dir(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let current = state.prefs.lock().unwrap().export_dir.clone();
    let dir = rfd::FileDialog::new()
        .set_directory(&current)
        .set_title("Dossier des MP4")
        .pick_folder()
        .ok_or_else(|| "annulé".to_string())?;
    let mut prefs = state.prefs.lock().unwrap();
    prefs.export_dir = dir.clone();
    prefs.save().map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

#[tauri::command]
fn reset_export_dir(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let dir = streamcraft_core::default_export_dir();
    let mut prefs = state.prefs.lock().unwrap();
    prefs.export_dir = dir.clone();
    prefs.save().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&dir);
    Ok(dir.display().to_string())
}

#[tauri::command]
fn preview_export_path(
    metadata: streamcraft_core::ExportMetadata,
    state: tauri::State<'_, AppState>,
) -> String {
    let prefs = state.prefs.lock().unwrap();
    let preset = metadata
        .naming_preset
        .unwrap_or(prefs.naming_preset);
    let template = if preset == streamcraft_core::ExportNamingPreset::Custom {
        prefs.resolved_naming_template()
    } else {
        preset.template().to_string()
    };
    let ctx = metadata.into_context("hls", "mp4");
    streamcraft_core::ExportNaming::relative_path(&template, &ctx)
}

#[tauri::command]
async fn av_sync_probe(path: String) -> Result<streamcraft_ffmpeg::MediaStreamTiming, String> {
    let ffmpeg = FFmpegPaths::resolve().map_err(|e| e.to_string())?;
    streamcraft_core::assemble::av_sync::probe(&ffmpeg, PathBuf::from(path).as_path())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn av_sync_realign(path: String) -> Result<streamcraft_core::assemble::av_sync::AvSyncResult, String> {
    let ffmpeg = FFmpegPaths::resolve().map_err(|e| e.to_string())?;
    streamcraft_core::assemble::av_sync::realign_like_player(&ffmpeg, PathBuf::from(path).as_path())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn av_sync_drift(
    path: String,
    end_audio_lead_seconds: Option<f64>,
) -> Result<streamcraft_core::assemble::av_sync::AvSyncResult, String> {
    let ffmpeg = FFmpegPaths::resolve().map_err(|e| e.to_string())?;
    streamcraft_core::assemble::av_sync::correct_av_drift(
        &ffmpeg,
        PathBuf::from(path).as_path(),
        end_audio_lead_seconds,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn av_sync_shift(
    path: String,
    audio_shift_seconds: f64,
) -> Result<streamcraft_core::assemble::av_sync::AvSyncResult, String> {
    let ffmpeg = FFmpegPaths::resolve().map_err(|e| e.to_string())?;
    streamcraft_core::assemble::av_sync::apply_manual_audio_shift(
        &ffmpeg,
        PathBuf::from(path).as_path(),
        audio_shift_seconds,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn cleanup_orphan_parts(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let export = state.prefs.lock().unwrap().export_dir.clone();
    let parts = export.join(".parts");
    if !parts.is_dir() {
        return Ok(0);
    }
    let mut n = 0usize;
    if let Ok(rd) = std::fs::read_dir(&parts) {
        for entry in rd.flatten() {
            let path = entry.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            n += 1;
        }
    }
    Ok(n)
}

#[tauri::command]
fn browser_navigate(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let mut normalized = url.trim().to_string();
    if !normalized.is_empty() && !normalized.starts_with("http://") && !normalized.starts_with("https://") {
        normalized = format!("https://{normalized}");
    }
    let parsed = Url::parse(&normalized).map_err(|e| e.to_string())?;
    if let Some(state) = app.try_state::<AppState>() {
        *state.page_url.lock().unwrap() = Some(parsed.to_string());
        *state.page_title.lock().unwrap() = None;
        // Keep detected media across same-series nav? Swift clears on explicit clear only.
    }
    let wv = app
        .get_webview("browser")
        .ok_or_else(|| "browser webview missing".to_string())?;
    wv.navigate(parsed).map_err(|e| e.to_string())
}

#[tauri::command]
fn browser_set_bounds(app: tauri::AppHandle, x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    let wv = app
        .get_webview("browser")
        .ok_or_else(|| "browser webview missing".to_string())?;
    wv.set_position(tauri::LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    wv.set_size(tauri::LogicalSize::new(width.max(1.0), height.max(1.0)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn browser_set_visible(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    let wv = app
        .get_webview("browser")
        .ok_or_else(|| "browser webview missing".to_string())?;
    if visible {
        wv.show().map_err(|e| e.to_string())
    } else {
        wv.hide().map_err(|e| e.to_string())
    }
}

/// Force a resource-timing rescan in the live WebView (same as NetworkObserver poll).
#[tauri::command]
fn browser_rescan(app: tauri::AppHandle) -> Result<(), String> {
    let wv = app
        .get_webview("browser")
        .ok_or_else(|| "browser webview missing".to_string())?;
    wv.eval(
        r#"(function(){
          try {
            var entries = performance.getEntriesByType('resource') || [];
            for (var i = 0; i < entries.length; i++) {
              var n = entries[i].name || '';
              if (!n) continue;
              window.__streamcraftMediaQueue = window.__streamcraftMediaQueue || [];
              window.__streamcraftMediaQueue.push({
                url: n, mime: null, source: 'resourceTiming', page: location.href
              });
            }
            document.querySelectorAll('video,audio,source').forEach(function(el){
              var s = el.currentSrc || el.src;
              if (s) window.__streamcraftMediaQueue.push({
                url: s, mime: el.type || null, source: 'mediaElement', page: location.href
              });
            });
          } catch (e) {}
        })();"#,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_download(
    media_id: String,
    metadata: Option<streamcraft_core::ExportMetadata>,
    variant_id: Option<String>,
    audio_track_id: Option<String>,
    subtitle_track_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<DownloadStartResult, String> {
    let media = {
        let g = state.detected.lock().unwrap();
        g.iter()
            .find(|m| m.id.to_string() == media_id)
            .cloned()
            .ok_or_else(|| "média introuvable — lance la lecture puis attends la détection".to_string())?
    };

    let prefs = state.prefs.lock().unwrap().clone();
    let _ = std::fs::create_dir_all(&prefs.export_dir);

    let variant_uuid = variant_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let audio_uuid = audio_track_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let subtitle_uuid = subtitle_track_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());

    let service = DownloadService::with_queue(prefs.clone(), state.queue.clone());
    let cookie = state
        .cookie_snapshot
        .lock()
        .unwrap()
        .cookie_header_for(&media.source_url);
    let referer = media
        .page_url
        .as_ref()
        .map(|u| u.to_string())
        .or_else(|| state.page_url.lock().unwrap().clone());
    let job_id = service
        .enqueue_media(media, metadata, variant_uuid, audio_uuid, subtitle_uuid)
        .await?;
    service
        .set_job_credentials(job_id, cookie, referer)
        .await;

    let queue = state.queue.clone();
    tauri::async_runtime::spawn(async move {
        let service = DownloadService::with_queue(prefs.clone(), queue.clone());
        if let Err(e) = service.run_job(job_id).await {
            tracing::error!(error = %e, %job_id, "download failed");
        }
        while let Some(next) = queue.next_queued().await {
            let svc = DownloadService::with_queue(prefs.clone(), queue.clone());
            if let Err(e) = svc.run_job(next.id).await {
                tracing::error!(error = %e, job_id = %next.id, "queued download failed");
            }
        }
    });

    Ok(DownloadStartResult {
        job_id: job_id.to_string(),
        message: "Téléchargement démarré".into(),
    })
}


fn spawn_queue_drain(prefs: AppPreferences, queue: Arc<DownloadQueue>) {
    tauri::async_runtime::spawn(async move {
        while let Some(next) = queue.next_queued().await {
            let svc = DownloadService::with_queue(prefs.clone(), queue.clone());
            if let Err(e) = svc.run_job(next.id).await {
                tracing::error!(error = %e, job_id = %next.id, "queued download failed");
            }
        }
    });
}

#[tauri::command]
async fn resume_job(job_id: String, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let id = Uuid::parse_str(&job_id).map_err(|e| e.to_string())?;
    let ok = state.queue.resume(id).await;
    if ok {
        let prefs = state.prefs.lock().unwrap().clone();
        spawn_queue_drain(prefs, state.queue.clone());
    }
    Ok(ok)
}

#[tauri::command]
async fn assemble_job(job_id: String, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let id = Uuid::parse_str(&job_id).map_err(|e| e.to_string())?;
    let ok = state.queue.request_assemble(id).await;
    if ok {
        let prefs = state.prefs.lock().unwrap().clone();
        spawn_queue_drain(prefs, state.queue.clone());
    }
    Ok(ok)
}

#[tauri::command]
fn list_raw_candidates(state: tauri::State<'_, AppState>) -> Vec<NetworkMediaCandidate> {
    state.raw_candidates.lock().unwrap().clone()
}

#[tauri::command]
fn list_bookmarks() -> Vec<streamcraft_core::store::bookmarks::Bookmark> {
    streamcraft_core::store::bookmarks::BookmarkStore::load().items
}

#[tauri::command]
fn toggle_bookmark(title: String, url: String) -> Result<bool, String> {
    let mut store = streamcraft_core::store::bookmarks::BookmarkStore::load();
    store.toggle(title, url)
}

#[tauri::command]
fn add_bookmark(title: String, url: String, as_hub: bool) -> Result<streamcraft_core::store::bookmarks::Bookmark, String> {
    let mut store = streamcraft_core::store::bookmarks::BookmarkStore::load();
    let final_url = if as_hub {
        streamcraft_core::store::bookmarks::BookmarkStore::hub_origin_url(&url).unwrap_or(url)
    } else {
        url
    };
    store.add(title, final_url)
}

#[tauri::command]
fn remove_bookmark(id: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mut store = streamcraft_core::store::bookmarks::BookmarkStore::load();
    store.remove(uuid)
}

#[tauri::command]
fn rename_bookmark(id: String, title: String) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mut store = streamcraft_core::store::bookmarks::BookmarkStore::load();
    store.rename(uuid, title)
}

#[tauri::command]
fn open_bookmark(id: String, app: tauri::AppHandle) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mut store = streamcraft_core::store::bookmarks::BookmarkStore::load();
    let url = store
        .items
        .iter()
        .find(|b| b.id == uuid)
        .map(|b| b.url.clone())
        .ok_or_else(|| "favori introuvable".to_string())?;
    let _ = store.mark_visited(uuid);
    browser_navigate(app, url)
}

#[derive(Serialize)]
struct CliPaths {
    inbox: String,
    via_app_addr: String,
}

#[tauri::command]
fn cli_paths() -> CliPaths {
    CliPaths {
        inbox: streamcraft_core::agent::via_app::inbox_dir().display().to_string(),
        via_app_addr: streamcraft_core::agent::via_app::default_addr(),
    }
}

#[derive(Serialize)]
struct BatchPreview {
    media_count: usize,
    page_count: usize,
    media: Vec<String>,
    pages: Vec<String>,
}

#[tauri::command]
fn preview_batch(text: String) -> BatchPreview {
    let urls = streamcraft_core::agent::batch::parse(&text);
    let part = streamcraft_core::agent::batch::partition(&urls);
    BatchPreview {
        media_count: part.media.len(),
        page_count: part.pages.len(),
        media: part.media.iter().map(|u| u.to_string()).collect(),
        pages: part.pages.iter().map(|u| u.to_string()).collect(),
    }
}

#[tauri::command]
async fn submit_batch(text: String, app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<String, String> {
    let urls = streamcraft_core::agent::batch::parse(&text);
    let part = streamcraft_core::agent::batch::partition(&urls);
    let prefs = state.prefs.lock().unwrap().clone();
    let service = DownloadService::with_queue(prefs.clone(), state.queue.clone());
    let snap = state.cookie_snapshot.lock().unwrap().clone();
    let mut enqueued = 0usize;
    for media_url in &part.media {
        let mut candidate = NetworkMediaCandidate::new(media_url.clone(), "batch");
        let page = state.page_url.lock().unwrap().clone().and_then(|u| Url::parse(&u).ok());
        candidate.page_url = page.clone();
        let title = state.page_title.lock().unwrap().clone();
        let class = MediaDetector::classify(media_url, None);
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
            .build()
            .ok();
        let media = match class {
            MediaClassification::Progressive => {
                Some(MediaDetector::make_progressive(&candidate, None, title.clone()))
            }
            MediaClassification::Hls => {
                if let Some(c) = client.as_ref() {
                    resolve_hls(c, &candidate, title.clone(), prefs.drm_gate).await.ok()
                        .or_else(|| Some(MediaDetector::make_hls_url(&candidate, title.clone())))
                } else {
                    Some(MediaDetector::make_hls_url(&candidate, title))
                }
            }
            MediaClassification::Dash => {
                if let Some(c) = client.as_ref() {
                    resolve_dash(c, &candidate, title, prefs.drm_gate).await.ok()
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(media) = media else { continue };
        let job_id = service.enqueue_media(media.clone(), None, None, None, None).await?;
        let cookie = snap.cookie_header_for(&media.source_url);
        let referer = media.page_url.as_ref().map(|u| u.to_string());
        service.set_job_credentials(job_id, cookie, referer).await;
        {
            let mut g = state.detected.lock().unwrap();
            if !g.iter().any(|m| m.source_url == media.source_url) {
                g.insert(0, media);
            }
        }
        enqueued += 1;
    }
    {
        let mut pages = state.pending_batch_pages.lock().unwrap();
        pages.clear();
        pages.extend(part.pages.iter().map(|u| u.to_string()));
    }
    if enqueued > 0 {
        spawn_queue_drain(prefs, state.queue.clone());
    }
    if let Some(first) = part.pages.first() {
        let _ = browser_navigate(app.clone(), first.to_string());
    }
    Ok(format!("{enqueued} média(s) en file, {} page(s) à ouvrir", part.pages.len()))
}

#[tauri::command]
fn cookie_refresh(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let wv = app
        .get_webview("browser")
        .ok_or_else(|| "browser webview missing".to_string())?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));
    let _ = wv.eval_with_callback(
        r#"(function(){ try { return JSON.stringify({ cookie: document.cookie || '', host: location.hostname || '', href: location.href || '' }); } catch(e) { return '{}'; } })()"#,
        move |json| {
            if let Ok(mut g) = tx.lock() {
                if let Some(sender) = g.take() {
                    let _ = sender.send(json);
                }
            }
        },
    );
    let json = rx.recv_timeout(std::time::Duration::from_millis(400)).unwrap_or_else(|_| "{}".into());
    #[derive(serde::Deserialize)]
    struct CookieJs { cookie: Option<String>, host: Option<String>, href: Option<String> }
    let parsed: CookieJs = serde_json::from_str(&json).unwrap_or(CookieJs { cookie: None, host: None, href: None });
    let host = parsed.host.unwrap_or_default();
    let raw = parsed.cookie.unwrap_or_default();
    let snap = streamcraft_core::session::SessionCredentialSnapshot::from_document_cookie(&raw, &host);
    let n = snap.cookies.len();
    *state.cookie_snapshot.lock().unwrap() = snap;
    if let Some(href) = parsed.href {
        if !href.is_empty() {
            *state.page_url.lock().unwrap() = Some(href);
        }
    }
    Ok(n)
}


#[tauri::command]
fn popup_policy(current: Option<String>, target: String, user_activated: bool) -> bool {
    let current = current.and_then(|u| Url::parse(&u).ok());
    let target = Url::parse(&target).ok();
    PopupGuard::should_open_blank_in_place(current.as_ref(), target.as_ref(), user_activated)
}

fn spawn_media_poller(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            let Some(state) = app.try_state::<AppState>() else { continue };
            // Native WKScriptMessageHandler payloads first (all frames) — never gated on JS eval.
            let mut batch = state.pending_media.lock().unwrap().drain(..).collect::<Vec<_>>();

            if let Some(wv) = app.get_webview("browser") {
                let (tx, rx) = tokio::sync::oneshot::channel::<String>();
                let tx = std::sync::Mutex::new(Some(tx));
                let _ = wv.eval_with_callback(PopupGuard::DRAIN_MEDIA_QUEUE_JS, move |json| {
                    if let Ok(mut g) = tx.lock() {
                        if let Some(sender) = g.take() {
                            let _ = sender.send(json);
                        }
                    }
                });
                if let Ok(json) = tokio::time::timeout(std::time::Duration::from_millis(200), rx).await {
                    if let Ok(json) = json {
                        if let Ok(payloads) = serde_json::from_str::<Vec<ScriptMediaPayload>>(&json) {
                            batch.extend(payloads);
                        }
                    }
                }
            }

            if batch.is_empty() {
                continue;
            }
            for payload in batch {
                ingest_script_payload(&app, &state, payload).await;
            }
        }
    });
}

async fn ingest_script_payload(
    app: &tauri::AppHandle,
    state: &AppState,
    payload: ScriptMediaPayload,
) {
    eprintln!("[ingest] start {}", payload.url);
    let Ok(url) = Url::parse(&payload.url) else { eprintln!("[ingest] bad url"); return };
    if !MediaURLHeuristics::looks_like_media(&url, payload.mime.as_deref()) {
        return;
    }
    let class = MediaDetector::classify(&url, payload.mime.as_deref());

    let page_url = payload
        .page
        .as_deref()
        .and_then(|p| Url::parse(p).ok())
        .or_else(|| {
            state
                .page_url
                .lock()
                .unwrap()
                .as_deref()
                .and_then(|p| Url::parse(p).ok())
        });

    let mut candidate = NetworkMediaCandidate::new(
        url.clone(),
        payload.source.clone().unwrap_or_else(|| "resourceTiming".into()),
    );
    candidate.mime_type = payload.mime.clone();
    candidate.page_url = page_url.clone();
    {
        let mut raw = state.raw_candidates.lock().unwrap();
        if !raw.iter().any(|c| c.url == candidate.url) {
            raw.insert(0, candidate.clone());
            if raw.len() > 200 {
                raw.truncate(200);
            }
        }
    }

    if matches!(class, MediaClassification::Segment | MediaClassification::Unknown) {
        eprintln!("[ingest] raw {:?}", class);
        return;
    }

    let key = url.as_str().to_string();
    {
        let mut seen = state.seen_urls.lock().unwrap();
        if seen.contains(&key) {
            return;
        }
        seen.insert(key);
    }

    let raw_page_title = state.page_title.lock().unwrap().clone();
    let title = Some(streamcraft_core::media::page_title::resolve_title(
        &[],
        raw_page_title.as_deref(),
        page_url.as_ref(),
        &url,
    ));

    let prefs = state.prefs.lock().unwrap().clone();
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
        .build()
        .ok();

    let media = match class {
        MediaClassification::Progressive => {
            Some(MediaDetector::make_progressive(&candidate, None, title.clone()))
        }
        MediaClassification::Hls => {
            if let Some(client) = client.as_ref() {
                match resolve_hls(client, &candidate, title.clone(), prefs.drm_gate).await {
                    Ok(m) => Some(m),
                    Err(_) => Some(MediaDetector::make_hls_url(&candidate, title.clone())),
                }
            } else {
                Some(MediaDetector::make_hls_url(&candidate, title))
            }
        }
        MediaClassification::Dash => {
            if let Some(client) = client.as_ref() {
                resolve_dash(client, &candidate, title, prefs.drm_gate)
                    .await
                    .ok()
            } else {
                None
            }
        }
        MediaClassification::Segment | MediaClassification::Unknown => None,
    };

    let Some(mut media) = media else { return };
    if media.page_url.is_none() {
        media.page_url = page_url;
    }

    {
        let mut g = state.detected.lock().unwrap();
        if !g.iter().any(|m| m.source_url == media.source_url) {
            g.insert(0, media.clone());
        } else {
            return;
        }
    }

    let _ = app.emit("media-detected", &media);
    eprintln!("[ingest] emitted {}", media.source_url);
    tracing::info!(url = %media.source_url, "network observer detected media");

    if prefs.should_auto_enqueue(media.kind) {
        let akey = media.source_url.as_str().to_string();
        let mut keys = state.auto_enqueued_keys.lock().unwrap();
        if !keys.contains(&akey) {
            // Skip empty HLS (no segments yet) like Swift
            let empty_hls = media.kind == streamcraft_core::ManifestKind::Hls
                && media.segment_count == Some(0);
            if !empty_hls {
                keys.insert(akey);
                drop(keys);
                let _ = app.emit("auto-enqueue-offer", &media);
            }
        }
    }
}

async fn resolve_hls(
    client: &reqwest::Client,
    candidate: &NetworkMediaCandidate,
    title: Option<String>,
    drm_gate: bool,
) -> Result<DetectedMedia, String> {
    let text = client
        .get(candidate.url.clone())
        .header(
            "Referer",
            candidate
                .page_url
                .as_ref()
                .map(|u| u.as_str())
                .unwrap_or(candidate.url.as_str()),
        )
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    if drm_gate && text.contains("#EXT-X-KEY") {
        let playlist = HLSParser::parse(&text, &candidate.url).map_err(|e| e.to_string())?;
        let mut media = MediaDetector::make_hls(candidate, &playlist, title);
        media.protection = streamcraft_core::MediaProtection::Drm {
            reason: "#EXT-X-KEY".into(),
        };
        return Ok(media);
    }

    let playlist = HLSParser::parse(&text, &candidate.url).map_err(|e| e.to_string())?;
    let mut media = MediaDetector::make_hls(candidate, &playlist, title);
    // Swift MediaCatalog: master → fetch preferred media for segments + duration
    if matches!(playlist.kind, streamcraft_core::media::hls::HLSPlaylistKind::Master) {
        if let Some(best) = media.preferred_variant().cloned() {
            let referer = candidate
                .page_url
                .as_ref()
                .map(|u| u.as_str())
                .unwrap_or(candidate.url.as_str())
                .to_string();
            if let Ok(resp) = client
                .get(best.playlist_url.clone())
                .header("Referer", &referer)
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(body) = resp.text().await {
                        if let Ok(media_pl) = HLSParser::parse(&body, &best.playlist_url) {
                            if let Some(m) = media_pl.media.as_ref() {
                                media.segment_count = Some(m.segments.len());
                                media.duration_seconds = Some(m.total_duration());
                                media.is_vod = m.has_end_list;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(media)
}

async fn resolve_dash(
    client: &reqwest::Client,
    candidate: &NetworkMediaCandidate,
    title: Option<String>,
    drm_gate: bool,
) -> Result<DetectedMedia, String> {
    let text = client
        .get(candidate.url.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let manifest = DASHParser::parse(&text, &candidate.url).map_err(|e| e.to_string())?;
    let variants: Vec<MediaVariant> = manifest
        .video_representations
        .iter()
        .map(|r| {
            let mut v = MediaVariant::new(
                r.segments
                    .first()
                    .map(|s| s.url.clone())
                    .unwrap_or_else(|| candidate.url.clone()),
            );
            v.bandwidth = r.bandwidth;
            if let (Some(w), Some(h)) = (r.width, r.height) {
                v.resolution.replace(Resolution { width: w, height: h });
            }
            v.codecs = r.codecs.clone();
            v.dash_representation_id = Some(r.id.clone());
            v
        })
        .collect();

    let drm = if drm_gate && manifest.has_content_protection {
        Some("ContentProtection".into())
    } else {
        None
    };
    Ok(MediaDetector::make_dash(
        candidate,
        variants,
        vec![],
        vec![],
        title,
        drm,
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let prefs = AppPreferences::load();
    let _ = std::fs::create_dir_all(&prefs.export_dir);
    let queue = DownloadQueue::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            detected: std::sync::Mutex::new(Vec::new()),
            seen_urls: std::sync::Mutex::new(HashSet::new()),
            prefs: std::sync::Mutex::new(prefs),
            queue,
            page_title: std::sync::Mutex::new(None),
            page_url: std::sync::Mutex::new(None),
            pending_media: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            raw_candidates: std::sync::Mutex::new(Vec::new()),
            cookie_snapshot: std::sync::Mutex::new(streamcraft_core::session::SessionCredentialSnapshot::empty()),
            auto_enqueued_keys: std::sync::Mutex::new(HashSet::new()),
            pending_batch_pages: std::sync::Mutex::new(Vec::new()),
        })
        .invoke_handler(tauri::generate_handler![
            get_preferences,
            save_preferences,
            ffmpeg_status,
            list_detected,
            clear_detected,
            start_download,
            list_jobs,
            cancel_job,
            remove_job,
            resume_job,
            assemble_job,
            list_raw_candidates,
            list_bookmarks,
            toggle_bookmark,
            add_bookmark,
            remove_bookmark,
            rename_bookmark,
            open_bookmark,
            cli_paths,
            preview_batch,
            submit_batch,
            cookie_refresh,
            list_history,
            open_path,
            reveal_exports,
            vpn_status,
            pick_export_dir,
            reset_export_dir,
            preview_export_path,
            cleanup_orphan_parts,
            av_sync_probe,
            av_sync_realign,
            av_sync_drift,
            av_sync_shift,
            popup_policy,
            browser_navigate,
            browser_set_bounds,
            browser_set_visible,
            browser_rescan,
        ])
        .setup(|app| {
            let window = app
                .get_window("main")
                .ok_or_else(|| "main window missing".to_string())?;

            let browser = tauri::webview::WebviewBuilder::new(
                "browser",
                WebviewUrl::External("about:blank".parse().unwrap()),
            )
            .initialization_script_for_all_frames(PopupGuard::MEDIA_HOOKS_JS)
            .initialization_script_for_all_frames(PopupGuard::NEUTRALIZE_WINDOW_OPEN_JS)
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
            )
            .on_navigation(|url| {
                // Mirror PopupGuard: allow http(s) only
                matches!(url.scheme(), "http" | "https" | "about")
            })
            .on_document_title_changed(|webview, title| {
                if let Some(state) = webview.app_handle().try_state::<AppState>() {
                    *state.page_title.lock().unwrap() = Some(title);
                }
            });

            let browser_wv = window.add_child(
                browser,
                tauri::LogicalPosition::new(360.0, 52.0),
                tauri::LogicalSize::new(900.0, 500.0),
            )?;

            let handle = app.handle().clone();
            #[cfg(target_os = "macos")]
            {
                let pending = app.state::<AppState>().pending_media.clone();
                let handle2 = handle.clone();
                let _ = browser_wv.with_webview(move |platform| {
                    match macos_media_hook::attach(platform.inner(), pending) {
                        Some(_) => tracing::info!("streamcraftMedia WKScriptMessageHandler attached"),
                        None => tracing::error!("streamcraftMedia attach failed"),
                    }
                    // Navigate only after handler is live (all-frames posts).
                    let _ = browser_navigate(
                        handle2,
                        "https://fs016.lol/15140968-visionquest-saison-1-2026.html".into(),
                    );
                });
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = browser_navigate(
                    handle.clone(),
                    "https://fs016.lol/15140968-visionquest-saison-1-2026.html".into(),
                );
            }

            spawn_media_poller(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running OpenStream");
}
