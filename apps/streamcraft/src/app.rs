use std::collections::HashSet;
use std::sync::Arc;

use egui::{RichText, Sense};
use streamcraft_core::download::queue::DownloadQueue;
use streamcraft_core::download::service::DownloadService;
use streamcraft_core::media::cdn_gate::{
    hls_capture_score, needs_player_unlock, same_stream_family,
};
use streamcraft_core::media::heuristics::MediaURLHeuristics;
use streamcraft_core::media::models::{DetectedMedia, DownloadJob};
use streamcraft_core::session::SessionCredentialSnapshot;
use streamcraft_core::store::prefs::AppPreferences;
use streamcraft_ffmpeg::FFmpegPaths;
use uuid::Uuid;

use crate::brand;
use crate::browser::{BrowserMessage, EmbeddedBrowser};
use crate::detect::{self, IngestInput};
use crate::panels::{detected, download_sheet, queue, settings};
use crate::panels::download_sheet::DownloadSheet;
use crate::roles;
use crate::sidebar::{self, NavTab};
use crate::theme::{self, BG, MUTED};

enum AppEvent {
    Detected(DetectedMedia),
    Status(String),
    Jobs(Vec<DownloadJob>),
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
    jobs_cache: Vec<DownloadJob>,
    status_line: String,
    ffmpeg_ok: bool,
    ffmpeg_source: String,
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
    pending_navigate: Option<String>,
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
            jobs_cache: Vec::new(),
            status_line: if ffmpeg_ok {
                format!("Ready | FFmpeg {ffmpeg_source}")
            } else {
                "FFmpeg missing — run ./scripts/fetch-ffmpeg.sh".into()
            },
            ffmpeg_ok,
            ffmpeg_source,
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
            pending_navigate: start_url,
        }
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
        let _ = browser.set_visible(show && self.sheet.is_none());
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
                .enqueue_media(media, metadata, variant, audio, subtitle)
                .await
            {
                Ok(job_id) => {
                    service
                        .set_job_credentials(job_id, cookie, referer)
                        .await;
                    if let Err(e) = service.run_job(job_id).await {
                        tracing::error!(error = %e, %job_id, "download failed");
                        let _ = tx.send(AppEvent::Status(format!("Failed: {e}")));
                    } else {
                        let _ = tx.send(AppEvent::Status("Download completed".into()));
                    }
                    // Drain remaining queued jobs (concurrency = sequential chain for MVP)
                    while let Some(next) = q.next_queued().await {
                        let svc = DownloadService::with_queue(prefs.clone(), q.clone());
                        if let Err(e) = svc.run_job(next.id).await {
                            tracing::error!(error = %e, job_id = %next.id, "queued download failed");
                        }
                    }
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
                    self.page_url = Some(u.clone());
                    if self.url_input.trim().is_empty() || self.url_input.starts_with("http") {
                        // keep bar in sync when navigating in-page
                    }
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
                AppEvent::Detected(media) => {
                    if self
                        .detected
                        .iter()
                        .any(|m| m.source_url == media.source_url)
                    {
                        continue;
                    }
                    // Prefer player-unlocked media (`gk=`) over cold master of same CDN family.
                    if let Some(idx) = self.detected.iter().position(|m| {
                        same_stream_family(&m.source_url, &media.source_url)
                    }) {
                        let old_score = hls_capture_score(&self.detected[idx].source_url);
                        let new_score = hls_capture_score(&media.source_url);
                        if new_score <= old_score {
                            continue;
                        }
                        self.detected.remove(idx);
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
                AppEvent::Jobs(jobs) => self.jobs_cache = jobs,
            }
        }
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
                let prefs = self.prefs.clone();
                let tx = self.event_tx.clone();
                self.rt.spawn(async move {
                    if q.resume(id).await {
                        let svc = DownloadService::with_queue(prefs.clone(), q.clone());
                        if let Err(e) = svc.run_job(id).await {
                            let _ = tx.send(AppEvent::Status(format!("Resume failed: {e}")));
                        }
                        while let Some(next) = q.next_queued().await {
                            let svc = DownloadService::with_queue(prefs.clone(), q.clone());
                            let _ = svc.run_job(next.id).await;
                        }
                    }
                });
                self.status_line = "Resumed".into();
            }
            queue::QueueAction::Assemble(id) => {
                let q = self.queue.clone();
                let prefs = self.prefs.clone();
                let tx = self.event_tx.clone();
                self.rt.spawn(async move {
                    if q.request_assemble(id).await {
                        let svc = DownloadService::with_queue(prefs, q);
                        if let Err(e) = svc.run_job(id).await {
                            let _ = tx.send(AppEvent::Status(format!("Assemble failed: {e}")));
                        } else {
                            let _ = tx.send(AppEvent::Status("Assembled".into()));
                        }
                    }
                });
            }
            queue::QueueAction::Remove(id) => {
                let q = self.queue.clone();
                self.rt.spawn(async move {
                    let _ = q.remove(id).await;
                });
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
        self.drain_browser();
        self.drain_events();
        self.poll_jobs();
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(0)))
            .show(ui, |ui| match self.tab {
                NavTab::Browse => {
                    browser_rect = Some(browse_with_webview(ui, self));
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

        self.handle_queue_action(queue_action);

        if let Some(rect) = browser_rect {
            if rect.width() > 32.0 && rect.height() > 32.0 {
                self.sync_browser_surface(frame, rect, show_browser);
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
    }
}

fn browse_with_webview(ui: &mut egui::Ui, app: &mut StreamCraftApp) -> egui::Rect {
    egui::Panel::top("browse_chrome")
        .exact_size(56.0)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(12.0);
                let field_w = (ui.available_width() - 280.0).max(200.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.url_input)
                        .desired_width(field_w)
                        .hint_text("https://… page or stream URL")
                        .margin(egui::Margin::symmetric(12, 8)),
                );
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
                if theme::ghost_button(ui, "URL↓").clicked() {
                    app.enqueue_media_url();
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
