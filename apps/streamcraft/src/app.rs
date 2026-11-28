use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use egui::{RichText, Sense};
use streamcraft_core::download::queue::DownloadQueue;
use streamcraft_core::download::service::{DownloadService, JobCredentials};
use streamcraft_core::media::cdn_gate::{
    needs_player_unlock, pick_capture, same_stream_family,
};
use streamcraft_core::media::heuristics::MediaURLHeuristics;
use streamcraft_core::media::models::{DetectedMedia, DownloadJob, DownloadState};
use streamcraft_core::session::SessionCredentialSnapshot;
use streamcraft_core::store::bookmarks::BookmarkStore;
use streamcraft_core::store::prefs::AppPreferences;
use streamcraft_ffmpeg::FFmpegPaths;
use uuid::Uuid;

use crate::brand;
use crate::browser::{BrowserMessage, EmbeddedBrowser};
use crate::detect::{self, IngestInput};
use crate::panels::{bookmarks, detected, download_sheet, queue, settings, sync_sheet};
use crate::panels::bookmarks::BookmarksUi;
use crate::panels::download_sheet::DownloadSheet;
use crate::roles;
use crate::sidebar::{self, NavTab};
use crate::theme::{self, ACCENT, BG, MUTED};

enum AppEvent {
    Detected(DetectedMedia),
    Status(String),
    Jobs(Vec<DownloadJob>),
    /// Queue state changed: start workers if slots are free.
    Pump,
    /// Probe result for the sync sheet's file.
    SyncTiming(std::path::PathBuf, Option<streamcraft_ffmpeg::MediaStreamTiming>),
    /// A sync operation finished (message for the sheet).
    SyncDone(std::path::PathBuf, String),
    /// FFmpeg install progress line.
    FfmpegProgress(String),
    /// FFmpeg install finished: resolved source, or the error.
    FfmpegInstalled(Result<String, String>),
}

pub struct StreamCraftApp {
    tab: NavTab,
    sidebar_expanded: bool,
    brand_icon: egui::TextureHandle,
    url_input: String,
    prefs: AppPreferences,
    detected: Vec<DetectedMedia>,
    seen_urls: HashSet<String>,
    auto_enqueued: HashSet<String>,
    queue: Arc<DownloadQueue>,
    active_workers: Arc<AtomicUsize>,
    jobs_cache: Vec<DownloadJob>,
    status_line: String,
    ffmpeg_ok: bool,
    ffmpeg_source: String,
    /// Some(progress line) while the on-demand FFmpeg install runs.
    ffmpeg_installing: Option<String>,
    browser: Option<EmbeddedBrowser>,
    browser_tx: Option<std::sync::mpsc::Sender<BrowserMessage>>,
    browser_rx: Option<std::sync::mpsc::Receiver<BrowserMessage>>,
    event_tx: std::sync::mpsc::Sender<AppEvent>,
    event_rx: std::sync::mpsc::Receiver<AppEvent>,
    rt: tokio::runtime::Runtime,
    roles_count: usize,
    last_browser_bounds: Option<egui::Rect>,
    page_url: Option<String>,
    page_title: Option<String>,
    cookie_snapshot: SessionCredentialSnapshot,
    sheet: Option<DownloadSheet>,
    sync: Option<sync_sheet::SyncSheet>,
    pending_navigate: Option<String>,
    bookmarks: BookmarkStore,
    bookmarks_ui: BookmarksUi,
    /// Previous nav tab — reload favorites from disk when entering Favorites.
    prev_tab: NavTab,
}

impl StreamCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(target_os = "macos")]
        crate::macos_dock_icon::ensure_applied();

        let prefs = AppPreferences::load();
        let _ = std::fs::create_dir_all(&prefs.export_dir);
        let (ffmpeg_ok, ffmpeg_source) = match FFmpegPaths::resolve() {
            Ok(p) => (true, short_source(&p.source)),
            Err(_) => (false, "missing".into()),
        };
        let roles_count = roles::streamcraft_roles().len();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        let (btx, brx) = std::sync::mpsc::channel();
        let (etx, erx) = std::sync::mpsc::channel();
        // Agent / smoke: STREAMCRAFT_START_URL=https://… opens after webview create.
        let start_url = std::env::var("STREAMCRAFT_START_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Self {
            tab: NavTab::Browse,
            sidebar_expanded: true,
            brand_icon: brand::load_sidebar_texture(&cc.egui_ctx),
            url_input: start_url.clone().unwrap_or_default(),
            prefs,
            detected: Vec::new(),
            seen_urls: HashSet::new(),
            auto_enqueued: HashSet::new(),
            queue: DownloadQueue::new(),
            active_workers: Arc::new(AtomicUsize::new(0)),
            jobs_cache: Vec::new(),
            status_line: if ffmpeg_ok {
                format!("Ready | FFmpeg {ffmpeg_source}")
            } else {
                "FFmpeg missing — run ./scripts/fetch-ffmpeg.sh".into()
            },
            ffmpeg_ok,
            ffmpeg_source,
            ffmpeg_installing: None,
            browser: None,
            browser_tx: Some(btx),
            browser_rx: Some(brx),
            event_tx: etx,
            event_rx: erx,
            rt,
            roles_count,
            last_browser_bounds: None,
            page_url: None,
            page_title: None,
            cookie_snapshot: SessionCredentialSnapshot::empty(),
            sheet: None,
            sync: None,
            pending_navigate: start_url,
            bookmarks: BookmarkStore::load(),
            bookmarks_ui: BookmarksUi::default(),
            prev_tab: NavTab::Browse,
        }
    }

    fn current_page_url(&self) -> String {
        self.page_url
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| self.url_input.trim().to_string())
    }

    fn toggle_current_bookmark(&mut self) {
        let url = self.current_page_url();
        if url.is_empty() {
            self.status_line = "Open a page before starring".into();
            return;
        }
        let title = self.page_title.clone().unwrap_or_default();
        match self.bookmarks.toggle(title, url) {
            Ok(true) => self.status_line = "Added to favorites".into(),
            Ok(false) => self.status_line = "Removed from favorites".into(),
            Err(e) => self.status_line = format!("Favorite failed: {e}"),
        }
    }

    fn history_step(&mut self, back: bool) {
        if let Some(b) = &self.browser {
            if let Err(e) = b.history_step(back) {
                self.status_line = format!("Navigation failed: {e}");
            }
        }
    }

    fn open_bookmark_url(&mut self, url: String) {
        self.url_input = url;
        self.tab = NavTab::Browse;
        self.navigate();
    }

    fn ensure_browser(&mut self, frame: &eframe::Frame, bounds: egui::Rect) {
        if self.browser.is_some() {
            return;
        }
        let Some(tx) = self.browser_tx.clone() else {
            return;
        };
        match EmbeddedBrowser::create(frame, tx, bounds, self.prefs.block_redirects) {
            Ok(b) => {
                self.browser = Some(b);
                self.status_line = "In-app browser ready".into();
                if let Some(url) = self.pending_navigate.take() {
                    if let Some(browser) = &self.browser {
                        let _ = browser.navigate(&url);
                        self.status_line = format!("Loading {url}");
                    }
                }
            }
            Err(e) => {
                self.status_line = format!("Browser failed: {e}");
            }
        }
    }

    fn sync_browser_surface(&mut self, frame: &eframe::Frame, bounds: egui::Rect, show: bool) {
        if show {
            self.ensure_browser(frame, bounds);
        }
        let Some(browser) = &self.browser else {
            return;
        };
        let _ = browser.set_visible(show && self.sheet.is_none() && self.sync.is_none());
        if show {
            if self.last_browser_bounds != Some(bounds) {
                let _ = browser.set_bounds(bounds);
                self.last_browser_bounds = Some(bounds);
            }
        }
    }

    fn navigate(&mut self) {
        let mut url = self.url_input.trim().to_string();
        if url.is_empty() {
            self.status_line = "Enter a URL first".into();
            return;
        }
        if !url.contains("://") {
            url = format!("https://{url}");
            self.url_input = url.clone();
        }
        if let Ok(parsed) = url::Url::parse(&url) {
            if MediaURLHeuristics::looks_like_media(&parsed, None) {
                self.enqueue_media_url();
                return;
            }
        }
        if let Some(browser) = &self.browser {
            match browser.navigate(&url) {
                Ok(()) => {
                    self.page_url = Some(url.clone());
                    self.status_line = format!("Loading {url}");
                }
                Err(e) => self.status_line = format!("Navigate failed: {e}"),
            }
        } else {
            self.pending_navigate = Some(url);
            self.status_line = "Starting browser…".into();
        }
    }

    fn enqueue_media_url(&mut self) {
        let raw = self.url_input.trim().to_string();
        if raw.is_empty() {
            self.status_line = "Paste a .m3u8 / .mpd / .mp4 URL".into();
            return;
        }
        let drm_gate = self.prefs.drm_gate;
        let tx = self.event_tx.clone();
        self.status_line = "Resolving media…".into();
        self.rt.spawn(async move {
            match detect::resolve_direct_url(&raw, drm_gate).await {
                Ok(media) => {
                    let _ = tx.send(AppEvent::Detected(media));
                    let _ = tx.send(AppEvent::Status("Media resolved — pick Download".into()));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Status(format!("Resolve failed: {e}")));
                }
            }
        });
    }

    fn open_download_sheet(&mut self, media: DetectedMedia) {
        self.sheet = Some(DownloadSheet::new(media, &self.prefs));
        if let Some(browser) = &self.browser {
            let _ = browser.set_visible(false);
        }
    }

    fn start_download_from_sheet(&mut self) {
        let Some(sheet) = self.sheet.take() else {
            return;
        };
        let media = sheet.media;
        let metadata = Some(sheet.metadata);
        let variant = sheet.variant_id;
        let audio = sheet.audio_id;
        let subtitle = sheet.subtitle_id;
        let prefs = self.prefs.clone();
        let q = self.queue.clone();
        let cookie = self
            .cookie_snapshot
            .cookie_header_for(&media.source_url);
        let referer = media
            .page_url
            .as_ref()
            .map(|u| u.to_string())
            .or_else(|| self.page_url.clone());
        let tx = self.event_tx.clone();
        self.status_line = format!("Downloading {}", media.source_url);
        self.tab = NavTab::Queue;
        self.rt.spawn(async move {
            let service = DownloadService::with_queue(prefs.clone(), q.clone());
            match service
                .enqueue_media(
                    media,
                    metadata,
                    variant,
                    audio,
                    subtitle,
                    JobCredentials {
                        cookie_header: cookie,
                        referer,
                    },
                )
                .await
            {
                Ok(_) => {
                    let _ = tx.send(AppEvent::Pump);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Status(format!("Enqueue failed: {e}")));
                }
            }
        });
    }

    fn spawn_ingest(&mut self, url: String, mime: Option<String>, source: Option<String>, page: Option<String>) {
        if !self.seen_urls.insert(url.clone()) {
            return;
        }
        tracing::debug!(%url, source = source.as_deref().unwrap_or("-"), "ingest candidate");
        let input = IngestInput {
            url,
            mime,
            source,
            page,
            page_title: self.page_title.clone(),
            fallback_page_url: self.page_url.clone(),
            drm_gate: self.prefs.drm_gate,
        };
        let tx = self.event_tx.clone();
        self.rt.spawn(async move {
            if let Some(media) = detect::ingest_candidate(input).await {
                tracing::info!(url = %media.source_url, "Detected");
                let _ = tx.send(AppEvent::Detected(media));
            }
        });
    }

    fn refresh_cookies(&mut self) {
        if let Some(browser) = &self.browser {
            self.cookie_snapshot = browser.cookie_snapshot(self.page_url.as_deref());
        }
    }

    fn drain_browser(&mut self) {
        // Native all-frames handler first (Swift/Tauri), then JS queue / ipc fallback.
        let mut msgs: Vec<_> = if let Some(browser) = &self.browser {
            let m = browser.take_pending_media();
            browser.drain_queue();
            m
        } else {
            Vec::new()
        };
        if let Some(rx) = &self.browser_rx {
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
        }
        for msg in msgs {
            match msg {
                BrowserMessage::Title(t) => {
                    self.page_title = Some(t.clone());
                    self.status_line = t;
                }
                BrowserMessage::PageUrl(u) => {
                    // Keep the bar on the page actually shown (links, Back/Forward).
                    self.url_input = u.clone();
                    self.page_url = Some(u);
                    self.refresh_cookies();
                }
                BrowserMessage::Candidate {
                    url,
                    mime,
                    source,
                    page,
                } => {
                    self.spawn_ingest(url, mime, source, page);
                }
                BrowserMessage::Error(e) => self.status_line = e,
            }
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AppEvent::Detected(mut media) => {
                    if self
                        .detected
                        .iter()
                        .any(|m| m.source_url == media.source_url)
                    {
                        continue;
                    }
                    // Same CDN stream seen twice: keep the best capture (never audio-only
                    // over video) carrying the player's `gk` unlock.
                    if let Some(idx) = self.detected.iter().position(|m| {
                        same_stream_family(&m.source_url, &media.source_url)
                    }) {
                        let old = self.detected.remove(idx);
                        let (keep, changed) = pick_capture(old, media);
                        if !changed {
                            self.detected.insert(idx, keep);
                            continue;
                        }
                        media = keep;
                    }
                    let locked = needs_player_unlock(&media.source_url);
                    let auto = self.prefs.should_auto_enqueue(media.kind) && !locked;
                    let empty_hls = media.kind == streamcraft_core::ManifestKind::Hls
                        && media.segment_count == Some(0);
                    let key = media.source_url.as_str().to_string();
                    self.detected.insert(0, media.clone());
                    self.status_line = if locked {
                        format!(
                            "Detected HLS master (waiting player unlock) — {}",
                            media
                                .suggested_title
                                .clone()
                                .unwrap_or_else(|| media.source_url.to_string())
                        )
                    } else {
                        format!(
                            "Detected {} — {}",
                            detect::kind_label(media.kind),
                            media
                                .suggested_title
                                .clone()
                                .unwrap_or_else(|| media.source_url.to_string())
                        )
                    };
                    self.refresh_cookies();
                    if auto && !empty_hls && self.auto_enqueued.insert(key) {
                        if self.prefs.ask_quality || self.sheet.is_none() {
                            self.open_download_sheet(media);
                        }
                    }
                }
                AppEvent::Status(s) => self.status_line = s,
                AppEvent::Jobs(jobs) => {
                    self.jobs_cache = jobs;
                    // Self-heals a job enqueued while every worker was exiting.
                    if self.jobs_cache.iter().any(|j| j.state == DownloadState::Queued) {
                        self.pump();
                    }
                }
                AppEvent::Pump => self.pump(),
                AppEvent::FfmpegProgress(m) => self.ffmpeg_installing = Some(m),
                AppEvent::FfmpegInstalled(r) => {
                    self.ffmpeg_installing = None;
                    match r {
                        Ok(source) => {
                            self.ffmpeg_ok = true;
                            self.ffmpeg_source = short_source(&source);
                            self.status_line = "FFmpeg installé".into();
                        }
                        Err(e) => self.status_line = format!("Installation FFmpeg échouée : {e}"),
                    }
                }
                AppEvent::SyncTiming(path, timing) => {
                    if let Some(s) = self.sync.as_mut().filter(|s| s.path == path) {
                        s.timing = timing;
                    }
                }
                AppEvent::SyncDone(path, msg) => {
                    self.status_line = msg.clone();
                    if let Some(s) = self.sync.as_mut().filter(|s| s.path == path) {
                        s.busy = false;
                        s.status = Some(msg);
                        self.probe_sync(path);
                    }
                }
            }
        }
    }

    /// Scheduler: top up workers to `max_concurrent_jobs` (re-read each call). Each worker
    /// claims jobs atomically until none are queued, then exits.
    fn pump(&mut self) {
        let max = self.prefs.max_concurrent_jobs.max(1);
        while self.active_workers.load(Ordering::SeqCst) < max {
            self.active_workers.fetch_add(1, Ordering::SeqCst);
            let q = self.queue.clone();
            let prefs = self.prefs.clone();
            let tx = self.event_tx.clone();
            let active = self.active_workers.clone();
            self.rt.spawn(async move {
                let svc = DownloadService::with_queue(prefs, q.clone());
                while let Some(id) = q.claim_next().await {
                    let msg = match svc.run_job(id).await {
                        Ok(_) => "Download completed".to_string(),
                        Err(e) => {
                            tracing::error!(error = %e, job_id = %id, "download failed");
                            format!("Failed: {e}")
                        }
                    };
                    let _ = tx.send(AppEvent::Status(msg));
                }
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }

    /// User clicked "Installer FFmpeg": download + verify in the background.
    fn install_ffmpeg(&mut self) {
        self.ffmpeg_installing = Some("Préparation…".into());
        let tx = self.event_tx.clone();
        self.rt.spawn(async move {
            let ptx = tx.clone();
            let r = streamcraft_ffmpeg::install::install(move |m: &str| {
                let _ = ptx.send(AppEvent::FfmpegProgress(m.to_string()));
            })
            .await;
            let _ = tx.send(AppEvent::FfmpegInstalled(
                r.map(|p| p.source).map_err(|e| e.to_string()),
            ));
        });
    }

    fn probe_sync(&self, path: std::path::PathBuf) {
        let tx = self.event_tx.clone();
        self.rt.spawn(async move {
            let timing = match streamcraft_ffmpeg::FFmpegPaths::resolve() {
                Ok(ff) => ff.probe_stream_timing(&path).await.ok(),
                Err(_) => None,
            };
            let _ = tx.send(AppEvent::SyncTiming(path, timing));
        });
    }

    fn run_sync(&mut self, op: sync_sheet::SyncOp) {
        use streamcraft_core::assemble::av_sync;
        let Some(sheet) = self.sync.as_mut() else {
            return;
        };
        sheet.busy = true;
        sheet.status = Some("Traitement…".into());
        let path = sheet.path.clone();
        let tx = self.event_tx.clone();
        self.rt.spawn(async move {
            let msg = match streamcraft_ffmpeg::FFmpegPaths::resolve() {
                Err(e) => format!("FFmpeg introuvable : {e}"),
                Ok(ff) => {
                    let r = match op {
                        sync_sheet::SyncOp::PlayerAlign => av_sync::realign_like_player(&ff, &path).await,
                        sync_sheet::SyncOp::Drift(lead) => {
                            av_sync::correct_av_drift(&ff, &path, Some(lead)).await
                        }
                        sync_sheet::SyncOp::Shift(s) => {
                            av_sync::apply_manual_audio_shift(&ff, &path, s).await
                        }
                        sync_sheet::SyncOp::Auto => av_sync::repair_av_sync(&ff, &path).await,
                    };
                    match r {
                        Ok(res) => res.user_message(),
                        Err(e) => format!("Échec : {e}"),
                    }
                }
            };
            let _ = tx.send(AppEvent::SyncDone(path, msg));
        });
    }

    fn poll_jobs(&mut self) {
        let q = self.queue.clone();
        let tx = self.event_tx.clone();
        self.rt.spawn(async move {
            let jobs = q.list().await;
            let _ = tx.send(AppEvent::Jobs(jobs));
        });
    }

    fn handle_queue_action(&mut self, action: queue::QueueAction) {
        match action {
            queue::QueueAction::None => {}
            queue::QueueAction::Cancel(id) => {
                let q = self.queue.clone();
                self.rt.spawn(async move {
                    let _ = q.cancel(id).await;
                });
                self.status_line = format!("Cancelled {id}");
            }
            queue::QueueAction::Resume(id) => {
                let q = self.queue.clone();
                let tx = self.event_tx.clone();
                self.rt.spawn(async move {
                    if q.resume(id).await {
                        let _ = tx.send(AppEvent::Pump);
                    }
                });
                self.status_line = "Resumed".into();
            }
            queue::QueueAction::Assemble(id) => {
                let q = self.queue.clone();
                let tx = self.event_tx.clone();
                self.rt.spawn(async move {
                    if q.request_assemble(id).await {
                        let _ = tx.send(AppEvent::Pump);
                    }
                });
            }
            queue::QueueAction::Remove(id) => {
                let q = self.queue.clone();
                self.rt.spawn(async move {
                    let _ = q.remove(id).await;
                });
            }
            queue::QueueAction::Sync(path, title) => {
                self.sync = Some(sync_sheet::SyncSheet::new(path.clone(), title));
                self.probe_sync(path);
            }
            queue::QueueAction::Open(path) => {
                let _ = open::that(&path);
            }
        }
    }
}

impl eframe::App for StreamCraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        crate::macos_dock_icon::ensure_applied();
        // Linux: pump GTK for wry WebKitGTK (see linux_webview.rs). CachyOS Wayland report.
        #[cfg(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        crate::linux_webview::pump_gtk();
        self.drain_browser();
        self.drain_events();
        self.poll_jobs();
        // Linux: WebKitGTK only runs when pump_gtk does, i.e. once per egui frame.
        // At the 200 ms idle cadence pages loaded and scrolled at 5 fps.
        let gtk_live = cfg!(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )) && self.tab == NavTab::Browse
            && self.browser.is_some();
        ctx.request_repaint_after(std::time::Duration::from_millis(if gtk_live { 16 } else { 200 }));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Alt+←/→: page history, unless a text field is using the keys.
        if self.tab == NavTab::Browse && !ui.ctx().egui_wants_keyboard_input() {
            let (back, fwd) = ui.input(|i| {
                (
                    i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft),
                    i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight),
                )
            });
            if back || fwd {
                self.history_step(back);
            }
        }
        // Clipboard paste into URL field (Cmd/Ctrl+V when Browse)
        if self.tab == NavTab::Browse {
            let paste = ui.input(|i| {
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Paste(_)
                            if true
                    )
                })
            });
            if let Some(text) = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Paste(t) => Some(t.clone()),
                    _ => None,
                })
            }) {
                let t = text.trim().to_string();
                if !t.is_empty() && (t.starts_with("http") || t.contains("://") || t.contains('.'))
                {
                    // Only auto-fill if focus isn't elsewhere with selection — bar is primary
                    if self.url_input.is_empty() || paste {
                        self.url_input = t;
                    }
                }
            }
        }

        sidebar::show(
            ui,
            &mut self.tab,
            &mut self.sidebar_expanded,
            &self.brand_icon,
            self.ffmpeg_ok,
        );

        if self.tab == NavTab::Browse {
            egui::Panel::right("detected")
                .default_size(320.0)
                .size_range(260.0..=420.0)
                .show(ui, |ui| {
                    match detected::show(ui, &self.detected) {
                        detected::DetectedAction::Clear => {
                            self.detected.clear();
                            self.seen_urls.clear();
                        }
                        detected::DetectedAction::OpenDownload(i) => {
                            if let Some(m) = self.detected.get(i).cloned() {
                                self.open_download_sheet(m);
                            }
                        }
                        detected::DetectedAction::None => {}
                    }
                });
        }

        egui::Panel::bottom("status").exact_size(32.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(&self.status_line).color(MUTED).size(12.5));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{} roles", self.roles_count))
                            .color(MUTED)
                            .size(11.0),
                    );
                });
            });
        });

        let show_browser = self.tab == NavTab::Browse;
        let mut browser_rect = None;
        let mut queue_action = queue::QueueAction::None;
        let mut bookmarks_action = bookmarks::BookmarksAction::None;

        if !self.ffmpeg_ok {
            let mut install = false;
            egui::Panel::top("ffmpeg_banner")
                .frame(egui::Frame::new().fill(theme::ACCENT_DIM).inner_margin(egui::Margin::symmetric(16, 10)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("FFmpeg est introuvable : il sert à assembler les vidéos téléchargées.")
                                .color(theme::TEXT),
                        );
                        match &self.ffmpeg_installing {
                            Some(step) => {
                                ui.spinner();
                                ui.label(RichText::new(step).color(theme::MUTED));
                            }
                            None => {
                                if theme::primary_button(ui, "Installer FFmpeg").clicked() {
                                    install = true;
                                }
                            }
                        }
                    });
                });
            if install {
                self.install_ffmpeg();
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(0)))
            .show(ui, |ui| match self.tab {
                NavTab::Browse => {
                    browser_rect = Some(browse_with_webview(ui, self));
                }
                NavTab::Favorites => {
                    if self.prev_tab != NavTab::Favorites {
                        self.bookmarks = BookmarkStore::load();
                    }
                    let current = self.current_page_url();
                    let title = self.page_title.clone().unwrap_or_default();
                    bookmarks_action = bookmarks::show(
                        ui,
                        &mut self.bookmarks,
                        &mut self.bookmarks_ui,
                        &current,
                        &title,
                    );
                }
                NavTab::Queue => {
                    queue_action = queue::show(ui, &self.jobs_cache);
                }
                NavTab::Settings => {
                    settings::show(
                        ui,
                        &mut self.prefs,
                        self.ffmpeg_ok,
                        &self.ffmpeg_source,
                        &mut self.status_line,
                    );
                }
            });

        if let bookmarks::BookmarksAction::Open { url } = bookmarks_action {
            self.open_bookmark_url(url);
        }
        self.handle_queue_action(queue_action);
        self.prev_tab = self.tab;

        if let Some(rect) = browser_rect {
            if rect.width() > 32.0 && rect.height() > 32.0 {
                // wry gets physical pixels: X11 child webviews apply logical sizes
                // as pixels (only GTK's integer scale is divided out), so a 1.33
                // desktop scale left the webview at 75 % size and offset.
                let ppp = ui.ctx().pixels_per_point();
                self.sync_browser_surface(frame, rect * ppp, show_browser);
            }
        } else {
            self.sync_browser_surface(
                frame,
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
                false,
            );
        }

        // Download sheet overlay
        if self.sheet.is_some() {
            let action = {
                let sheet = self.sheet.as_mut().unwrap();
                download_sheet::show(ui, sheet, &self.prefs)
            };
            match action {
                download_sheet::SheetAction::Cancel => {
                    self.sheet = None;
                }
                download_sheet::SheetAction::Confirm => {
                    self.start_download_from_sheet();
                }
                download_sheet::SheetAction::None => {}
            }
        }

        // Sync sheet overlay
        if let Some(sheet) = self.sync.as_mut() {
            match sync_sheet::show(ui, sheet) {
                sync_sheet::SyncAction::Close => self.sync = None,
                sync_sheet::SyncAction::Run(op) => self.run_sync(op),
                sync_sheet::SyncAction::None => {}
            }
        }
    }
}

/// Five-point star: filled accent when `filled`, muted outline otherwise.
fn paint_star(painter: &egui::Painter, c: egui::Pos2, r: f32, filled: bool) {
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            c + egui::vec2(a.cos() * rr, a.sin() * rr)
        })
        .collect();
    if filled {
        // Concave: fill as triangles from the centre.
        for i in 0..10 {
            painter.add(egui::Shape::convex_polygon(
                vec![c, pts[i], pts[(i + 1) % 10]],
                ACCENT,
                egui::Stroke::NONE,
            ));
        }
    } else {
        painter.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.5, MUTED)));
    }
}

fn browse_with_webview(ui: &mut egui::Ui, app: &mut StreamCraftApp) -> egui::Rect {
    egui::Panel::top("browse_chrome")
        .exact_size(56.0)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(12.0);
                // Chevrons drawn too: no arrow glyphs in the bundled fonts either.
                for (tip, back) in [("Back (Alt+←)", true), ("Forward (Alt+→)", false)] {
                    let enabled = app.browser.is_some();
                    let b = ui.add_enabled(
                        enabled,
                        egui::Button::new("")
                            .fill(theme::ELEVATED)
                            .corner_radius(egui::CornerRadius::same(8))
                            .min_size(egui::vec2(32.0, 36.0)),
                    );
                    let c = b.rect.center();
                    let dx = if back { 3.0 } else { -3.0 };
                    ui.painter().add(egui::Shape::line(
                        vec![c + egui::vec2(dx, -6.0), c + egui::vec2(-dx, 0.0), c + egui::vec2(dx, 6.0)],
                        egui::Stroke::new(2.0, if enabled { theme::TEXT } else { MUTED }),
                    ));
                    if b.on_hover_text(tip).clicked() {
                        app.history_step(back);
                    }
                }
                let field_w = (ui.available_width() - 360.0).max(160.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.url_input)
                        .desired_width(field_w)
                        .hint_text("https://… page or stream URL")
                        .margin(egui::Margin::symmetric(12, 8)),
                );
                let page = app.current_page_url();
                let starred = !page.is_empty() && app.bookmarks.is_bookmarked(&page);
                // Drawn, not a glyph: the bundled fonts have no ★/☆ (showed as ▯).
                let star = ui.add(
                    egui::Button::new("")
                        .fill(theme::ELEVATED)
                        .corner_radius(egui::CornerRadius::same(8))
                        .min_size(egui::vec2(36.0, 36.0)),
                );
                paint_star(ui.painter(), star.rect.center(), 8.0, starred);
                if star
                    .on_hover_text(if starred {
                        "Remove from favorites"
                    } else {
                        "Add to favorites"
                    })
                    .clicked()
                {
                    app.toggle_current_bookmark();
                }
                if theme::primary_button(ui, "Go").clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    app.navigate();
                }
                if theme::ghost_button(ui, "Rescan").clicked() {
                    if let Some(b) = &app.browser {
                        match b.rescan() {
                            Ok(()) => app.status_line = "Rescanning page…".into(),
                            Err(e) => app.status_line = format!("Rescan failed: {e}"),
                        }
                    }
                }
                // ASCII label — egui default fonts lack "↓" (shows as □).
                if theme::ghost_button(ui, "Detect")
                    .on_hover_text("Treat the URL bar as a direct .m3u8 / .mpd / .mp4 stream")
                    .clicked()
                {
                    app.enqueue_media_url();
                }
                if theme::ghost_button(ui, "Annuaire").clicked() {
                    app.tab = NavTab::Favorites;
                }
            });
        });

    let (rect, _resp) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, COLOR32_HOLE);
    if app.browser.is_none() {
        let galley = ui.painter().layout_no_wrap(
            "Starting in-app browser…".into(),
            egui::FontId::proportional(14.0),
            MUTED,
        );
        let pos = egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(pos, galley, MUTED);
    }
    rect
}

const COLOR32_HOLE: egui::Color32 = egui::Color32::from_rgb(0x0a, 0x0a, 0x0c);

fn short_source(source: &str) -> String {
    if source.starts_with("bundled") {
        "bundled".into()
    } else if source.starts_with("installed") {
        "installed".into()
    } else if source == "path" {
        "PATH".into()
    } else if source == "env" {
        "env".into()
    } else {
        source.chars().take(24).collect()
    }
}

#[allow(dead_code)]
fn _uuid_unused(_: Uuid) {}
