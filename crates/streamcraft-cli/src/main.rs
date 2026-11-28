mod mcp;

use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use streamcraft_core::agent::catalog::{extract_news_id, fetch_series_catalog, resolve_embed_streams};
use streamcraft_core::agent::events::{AgentEvent, ExitCode};
use streamcraft_core::agent::flix::{
    digest_item_from_flix, fetch_rss_items, flix_data_dir, rss_url_for_hub, seen_path, FlixItem,
    FlixLastDigest, FlixSeenStore,
};
use streamcraft_core::agent::scrape::PageScraper;
use streamcraft_core::agent::season::enumerate_from_html;
use streamcraft_core::agent::via_app::{request as via_app_request, ViaAppRequest, ViaAppResponse};
use streamcraft_core::download::service::{DownloadService, JobCredentials};
use streamcraft_core::media::detector::MediaDetector;
use streamcraft_core::media::hls::HLSParser;
use streamcraft_core::media::models::{MediaClassification, MediaProtection, NetworkMediaCandidate};
use streamcraft_core::store::bookmarks::BookmarkStore;
use streamcraft_core::store::cli_jobs::{CliJobRecord, CliJobStore};
use streamcraft_core::store::prefs::AppPreferences;
use streamcraft_ffmpeg::FFmpegPaths;
use url::Url;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(
    name = "streamcraft",
    version,
    about = "StreamCraft headless CLI (JSON Lines)"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    /// Raw Cookie header (a=1; b=2)
    #[arg(long, global = true)]
    cookie: Option<String>,

    /// Path to a cookie file (raw header or Netscape first usable line)
    #[arg(long, global = true)]
    cookies: Option<PathBuf>,

    #[arg(long, global = true)]
    referer: Option<String>,

    #[arg(long, global = true, help = "Forward page fetch to running GUI via-app socket")]
    via_app: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Download a stream URL (.m3u8 / .mpd / .mp4). For a web page, use `fetch`.
    Download {
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Find the streams on a web page (player iframes included); --auto-download saves the first.
    Fetch {
        page_url: String,
        #[arg(long)]
        detect_only: bool,
        #[arg(long)]
        auto_download: bool,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Season {
        show_or_url: String,
        #[arg(long)]
        auto_download: bool,
        #[arg(long)]
        list_only: bool,
        #[arg(long)]
        season: Option<u32>,
        #[arg(long)]
        show: Option<String>,
    },
    Flix {
        #[command(subcommand)]
        cmd: FlixCmd,
    },
    Status {
        job_id: Option<String>,
    },
    List,
    Wait {
        job_id: String,
        #[arg(long, default_value_t = 3600)]
        timeout: u64,
    },
    Cancel {
        job_id: String,
    },
    Doctor {
        /// If FFmpeg is missing, download it (checksum-verified) into the user data dir.
        #[arg(long)]
        install_ffmpeg: bool,
    },
    /// Ping GUI via-app bridge
    Ping,
    /// Ask GUI to navigate (via-app Open) or print inbox hint
    Open {
        url: String,
    },
    /// Print inbox / via-app paths
    Inbox,
    /// MCP stdio server for AI agents (requires automation roots — D021)
    Mcp {
        /// Absolute folder an agent may read (repeatable)
        #[arg(long = "automation-read-root", required = true)]
        automation_read_root: Vec<PathBuf>,
        /// Absolute folder an agent may write (exports, cookies copies)
        #[arg(long = "automation-write-root", required = true)]
        automation_write_root: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum FlixCmd {
    Digest {
        #[arg(long)]
        feed: Vec<String>,
        #[arg(long)]
        mark_seen: bool,
        #[arg(long, default_value_t = 8)]
        max: usize,
    },
    Scan {
        #[arg(long)]
        feed: Vec<String>,
        #[arg(long, default_value_t = 8)]
        max: usize,
    },
    Bootstrap {
        #[arg(long)]
        feed: Vec<String>,
    },
    Seen {
        #[command(subcommand)]
        cmd: Option<SeenCmd>,
    },
    Pick {
        /// Indices from last digest, e.g. 1 or 1,3
        selection: String,
        #[arg(long)]
        enqueue: bool,
    },
}

#[derive(Subcommand, Debug)]
enum SeenCmd {
    List,
    Clear,
}

struct SessionOpts {
    cookie: Option<String>,
    referer: Option<String>,
    via_app: bool,
    json: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let opts = SessionOpts {
        cookie: resolve_cookie(cli.cookie.clone(), cli.cookies.as_ref()),
        referer: cli.referer.clone(),
        via_app: cli.via_app,
        json: cli.json,
    };
    let code = match run(cli, &opts).await {
        Ok(c) => c,
        Err(e) => {
            AgentEvent::Failed {
                job_id: None,
                message: format!("{e:#}"),
            }
            .emit();
            ExitCode::Internal
        }
    };
    std::process::exit(code as i32);
}

fn resolve_cookie(header: Option<String>, file: Option<&PathBuf>) -> Option<String> {
    if let Some(h) = header.filter(|s| !s.trim().is_empty()) {
        return Some(h);
    }
    let path = file?;
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Raw header file
    if !trimmed.contains('\t') && trimmed.contains('=') {
        return Some(trimmed.lines().next()?.trim().to_string());
    }
    // Netscape: name\tvalue on last columns — build simple header from name/value cols
    let mut parts = Vec::new();
    for line in trimmed.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() >= 7 {
            parts.push(format!("{}={}", cols[5], cols[6]));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

async fn run(cli: Cli, opts: &SessionOpts) -> Result<ExitCode> {
    let mut prefs = AppPreferences::load();
    match cli.command {
        Commands::Doctor { install_ffmpeg } => match FFmpegPaths::resolve() {
            Err(_) if install_ffmpeg => {
                let progress = |m: &str| {
                    AgentEvent::Log { level: "info".into(), message: m.into() }.emit()
                };
                match streamcraft_ffmpeg::install::install(progress).await {
                    Ok(p) => {
                        AgentEvent::Log {
                            level: "info".into(),
                            message: format!("ffmpeg={} source={}", p.ffmpeg.display(), p.source),
                        }
                        .emit();
                        Ok(ExitCode::Ok)
                    }
                    Err(e) => {
                        AgentEvent::Failed { job_id: None, message: format!("FFmpeg install failed: {e}") }
                            .emit();
                        Ok(ExitCode::Ffmpeg)
                    }
                }
            }
            Ok(p) => {
                AgentEvent::Log {
                    level: "info".into(),
                    message: format!("ffmpeg={} source={}", p.ffmpeg.display(), p.source),
                }
                .emit();
                Ok(ExitCode::Ok)
            }
            Err(_) => {
                AgentEvent::Failed {
                    job_id: None,
                    message: "ffmpeg not found — run `streamcraft doctor --install-ffmpeg`".into(),
                }
                .emit();
                Ok(ExitCode::Ffmpeg)
            }
        },
        Commands::Ping => match via_app_request(ViaAppRequest::Ping).await {
            Ok(ViaAppResponse::Pong) => {
                AgentEvent::Log {
                    level: "info".into(),
                    message: "pong".into(),
                }
                .emit();
                Ok(ExitCode::Ok)
            }
            Ok(other) => {
                AgentEvent::Failed {
                    job_id: None,
                    message: format!("unexpected via-app response: {other:?}"),
                }
                .emit();
                Ok(ExitCode::Network)
            }
            Err(e) => {
                AgentEvent::Failed {
                    job_id: None,
                    message: format!("via-app unreachable: {e}"),
                }
                .emit();
                Ok(ExitCode::Network)
            }
        },
        Commands::Open { url } => {
            let _ = write_inbox_open(&url);
            match via_app_request(ViaAppRequest::Open { url: url.clone() }).await {
                Ok(ViaAppResponse::Pong)
                | Ok(ViaAppResponse::Opened { .. })
                | Ok(ViaAppResponse::Html { .. }) => {
                    AgentEvent::Log {
                        level: "info".into(),
                        message: format!("open requested: {url}"),
                    }
                    .emit();
                    Ok(ExitCode::Ok)
                }
                Ok(ViaAppResponse::Error { message }) => {
                    AgentEvent::Log {
                        level: "warn".into(),
                        message: format!("via-app open: {message} (inbox written)"),
                    }
                    .emit();
                    Ok(ExitCode::Ok)
                }
                Err(e) => {
                    AgentEvent::Log {
                        level: "warn".into(),
                        message: format!("via-app unreachable ({e}); inbox command written"),
                    }
                    .emit();
                    Ok(ExitCode::Ok)
                }
            }
        }
        Commands::Inbox => {
            let inbox = streamcraft_core::agent::via_app::inbox_dir();
            let addr = streamcraft_core::agent::via_app::default_addr();
            let payload = serde_json::json!({
                "inbox": inbox.display().to_string(),
                "via_app_addr": addr,
                "flix_dir": flix_data_dir().display().to_string(),
                "cli_jobs": CliJobStore::path().display().to_string(),
            });
            if opts.json {
                println!("{}", serde_json::to_string(&payload)?);
            } else {
                AgentEvent::Log {
                    level: "info".into(),
                    message: payload.to_string(),
                }
                .emit();
            }
            Ok(ExitCode::Ok)
        }
        Commands::Mcp {
            automation_read_root,
            automation_write_root,
        } => {
            let config = mcp::McpConfig::from_flags(automation_read_root, Some(automation_write_root))?;
            mcp::serve_stdio(config).await?;
            Ok(ExitCode::Ok)
        }
        Commands::Download { url, title, out } => {
            if let Some(dir) = out {
                prefs.export_dir = dir;
            }
            download_url(&prefs, &url, title, opts).await
        }
        Commands::Fetch {
            page_url,
            detect_only,
            auto_download,
            title,
            out,
        } => {
            if let Some(dir) = out {
                prefs.export_dir = dir;
            }
            cmd_fetch(&prefs, &page_url, detect_only, auto_download, title, opts).await
        }
        Commands::Season {
            show_or_url,
            auto_download,
            list_only,
            season,
            show,
        } => {
            cmd_season(
                &prefs,
                &show_or_url,
                auto_download && !list_only,
                season,
                show,
                opts,
            )
            .await
        }
        Commands::Flix { cmd } => cmd_flix(cmd, opts).await,
        Commands::Status { job_id } => {
            let store = CliJobStore::load();
            let jobs: Vec<serde_json::Value> = if let Some(id) = job_id {
                let uuid = Uuid::parse_str(&id)?;
                store
                    .get(uuid)
                    .map(|j| vec![serde_json::to_value(j).unwrap_or_default()])
                    .unwrap_or_default()
            } else {
                store
                    .jobs
                    .iter()
                    .map(|j| serde_json::to_value(j).unwrap_or_default())
                    .collect()
            };
            AgentEvent::Status { jobs }.emit();
            Ok(ExitCode::Ok)
        }
        Commands::List => {
            let store = CliJobStore::load();
            AgentEvent::Status {
                jobs: store
                    .jobs
                    .iter()
                    .map(|j| serde_json::to_value(j).unwrap_or_default())
                    .collect(),
            }
            .emit();
            Ok(ExitCode::Ok)
        }
        Commands::Wait { job_id, timeout } => {
            let uuid = Uuid::parse_str(&job_id)?;
            let deadline = Instant::now() + Duration::from_secs(timeout);
            loop {
                let store = CliJobStore::load();
                let Some(job) = store.get(uuid).cloned() else {
                    AgentEvent::Failed {
                        job_id: Some(job_id.clone()),
                        message: "job not found".into(),
                    }
                    .emit();
                    return Ok(ExitCode::NotFound);
                };
                AgentEvent::Progress {
                    job_id: job_id.clone(),
                    progress: job.progress,
                    state: job.state.clone(),
                }
                .emit();
                if matches!(
                    job.state.as_str(),
                    "completed" | "failed" | "cancelled"
                ) {
                    return Ok(if job.state == "completed" {
                        ExitCode::Ok
                    } else if job.state == "cancelled" {
                        ExitCode::Cancelled
                    } else {
                        ExitCode::Network
                    });
                }
                if Instant::now() >= deadline {
                    AgentEvent::Failed {
                        job_id: Some(job_id),
                        message: "wait timeout".into(),
                    }
                    .emit();
                    return Ok(ExitCode::Network);
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }
        Commands::Cancel { job_id } => {
            let uuid = Uuid::parse_str(&job_id)?;
            let ok = CliJobStore::update(|s| s.request_cancel(uuid)).unwrap_or(false);
            AgentEvent::Log {
                level: "info".into(),
                message: if ok {
                    format!("cancel requested for {job_id}")
                } else {
                    format!("cannot cancel {job_id} (missing or terminal)")
                },
            }
            .emit();
            Ok(if ok {
                ExitCode::Ok
            } else {
                ExitCode::NotFound
            })
        }
    }
}

fn write_inbox_open(url: &str) -> Result<()> {
    let dir = streamcraft_core::agent::via_app::inbox_dir();
    std::fs::create_dir_all(&dir)?;
    let cmd = serde_json::json!({
        "id": Uuid::new_v4().to_string(),
        "kind": "open",
        "url": url,
        "createdAt": Utc::now().to_rfc3339(),
    });
    let path = dir.join(format!("{}.json", Uuid::new_v4()));
    std::fs::write(path, serde_json::to_string_pretty(&cmd)?)?;
    Ok(())
}

fn resolve_feeds(explicit: Vec<String>) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit;
    }
    let bookmarks = BookmarkStore::load();
    let mut feeds = Vec::new();
    for b in &bookmarks.items {
        if b.is_hub() {
            if let Some(rss) = rss_url_for_hub(&b.url) {
                feeds.push(rss);
            }
        } else if let Some(origin) = BookmarkStore::hub_origin_url(&b.url) {
            // Prefer hub origin for catalog bookmarks
            if let Some(rss) = rss_url_for_hub(&origin) {
                if !feeds.contains(&rss) {
                    feeds.push(rss);
                }
            }
        }
    }
    feeds
}

async fn cmd_flix(cmd: FlixCmd, opts: &SessionOpts) -> Result<ExitCode> {
    let seen_path = seen_path();
    match cmd {
        FlixCmd::Seen { cmd } => {
            let mut seen = FlixSeenStore::load(&seen_path);
            match cmd.unwrap_or(SeenCmd::List) {
                SeenCmd::List => {
                    let payload = serde_json::json!({ "count": seen.seen_ids.len(), "ids": seen.seen_ids });
                    if opts.json {
                        println!("{}", serde_json::to_string(&payload)?);
                    } else {
                        AgentEvent::Log {
                            level: "info".into(),
                            message: format!("seen={}", seen.seen_ids.len()),
                        }
                        .emit();
                    }
                }
                SeenCmd::Clear => {
                    seen.seen_ids.clear();
                    let _ = seen.save(&seen_path);
                    if opts.json {
                        println!("{{\"cleared\":true}}");
                    } else {
                        AgentEvent::Log {
                            level: "info".into(),
                            message: "seen cleared".into(),
                        }
                        .emit();
                    }
                }
            }
            Ok(ExitCode::Ok)
        }
        FlixCmd::Bootstrap { feed } => {
            let feeds = resolve_feeds(feed);
            if feeds.is_empty() {
                bail!("no feeds — pass --feed or add hub bookmarks");
            }
            let mut seen = FlixSeenStore::load(&seen_path);
            let mut marked = 0usize;
            for f in &feeds {
                let hub = Url::parse(f)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_string()))
                    .unwrap_or_else(|| "hub".into());
                match fetch_rss_items(f, &hub).await {
                    Ok(list) => {
                        for it in list.into_iter().take(50) {
                            if !seen.contains(&it.id) {
                                seen.mark(&it.id);
                                marked += 1;
                            }
                        }
                    }
                    Err(e) => eprintln!("rss {f}: {e}"),
                }
            }
            let _ = seen.save(&seen_path);
            if opts.json {
                println!("{{\"bootstrap\":true,\"marked\":{marked}}}");
            } else {
                AgentEvent::Log {
                    level: "info".into(),
                    message: format!("bootstrap marked {marked}"),
                }
                .emit();
            }
            Ok(ExitCode::Ok)
        }
        FlixCmd::Scan { feed, max } => {
            let feeds = resolve_feeds(feed);
            let (items, hubs_ok, hubs_skipped) = collect_rss(&feeds).await;
            let capped: Vec<_> = items.into_iter().take(max).collect();
            if opts.json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "count": capped.len(),
                        "hubs_ok": hubs_ok,
                        "hubs_skipped": hubs_skipped,
                        "items": capped,
                    }))?
                );
            } else {
                for (i, it) in capped.iter().enumerate() {
                    println!("{}. {} ({})", i + 1, it.title, it.hub);
                }
            }
            Ok(ExitCode::Ok)
        }
        FlixCmd::Digest {
            feed,
            mark_seen,
            max,
        } => {
            let feeds = resolve_feeds(feed);
            if feeds.is_empty() {
                bail!("no feeds — pass --feed or add hub bookmarks");
            }
            let mut seen = FlixSeenStore::load(&seen_path);
            // First run auto-bootstrap
            if seen.seen_ids.is_empty() {
                eprintln!("flix: seen empty — auto bootstrap (count: 0)");
                for f in &feeds {
                    let hub = Url::parse(f)
                        .ok()
                        .and_then(|u| u.host_str().map(|h| h.to_string()))
                        .unwrap_or_else(|| "hub".into());
                    if let Ok(list) = fetch_rss_items(f, &hub).await {
                        for it in list.into_iter().take(50) {
                            seen.mark(&it.id);
                        }
                    }
                }
                let _ = seen.save(&seen_path);
                let empty = FlixLastDigest {
                    updated: Some(Utc::now()),
                    items: vec![],
                };
                let _ = empty.save();
                if opts.json {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "updated": Utc::now().to_rfc3339(),
                            "count": 0,
                            "items": {}
                        }))?
                    );
                }
                return Ok(ExitCode::Ok);
            }

            let (raw, _, _) = collect_rss(&feeds).await;
            let mut unseen: Vec<FlixItem> = raw
                .into_iter()
                .filter(|it| !seen.contains(&it.id))
                .collect();
            unseen.truncate(max);
            let digest_items: Vec<_> = unseen
                .iter()
                .enumerate()
                .map(|(i, it)| digest_item_from_flix(i + 1, it))
                .collect();
            let last = FlixLastDigest {
                updated: Some(Utc::now()),
                items: digest_items.clone(),
            };
            let _ = last.save();
            if mark_seen {
                for it in &unseen {
                    seen.mark(&it.id);
                }
                let _ = seen.save(&seen_path);
            }
            if opts.json {
                let mut map = serde_json::Map::new();
                for d in &digest_items {
                    map.insert(d.n.to_string(), serde_json::to_value(d)?);
                }
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "updated": Utc::now().to_rfc3339(),
                        "count": digest_items.len(),
                        "items": map,
                    }))?
                );
            } else if digest_items.is_empty() {
                // silence
            } else {
                println!("Flix — {} nouveautés", digest_items.len());
                for d in &digest_items {
                    println!("{}. {} ({})", d.n, d.display, d.source);
                }
                println!("Réponds 1 / 1,3 ou season « Show » --season N");
            }
            Ok(ExitCode::Ok)
        }
        FlixCmd::Pick { selection, enqueue } => {
            let last = FlixLastDigest::load();
            let indices = parse_pick_selection(&selection)?;
            let picked = last.pick(&indices);
            if picked.is_empty() {
                bail!("no matching items in last digest — run flix digest first");
            }
            if opts.json && !enqueue {
                println!("{}", serde_json::to_string(&picked)?);
                return Ok(ExitCode::Ok);
            }
            if !enqueue {
                for d in &picked {
                    println!("{}. {} → {} ({})", d.n, d.display, d.mode, d.url);
                }
                return Ok(ExitCode::Ok);
            }
            // enqueue: spawn season or fetch subprocess
            let bin = std::env::current_exe()?;
            for d in &picked {
                let status = if d.mode == "season" {
                    let show = d.show.clone().unwrap_or_else(|| d.title.clone());
                    let season = d.season.unwrap_or(1);
                    StdCommand::new(&bin)
                        .args([
                            "season",
                            &show,
                            "--season",
                            &season.to_string(),
                            "--auto-download",
                        ])
                        .arg("--json")
                        .status()?
                } else {
                    StdCommand::new(&bin)
                        .args(["fetch", &d.url, "--auto-download", "--json"])
                        .status()?
                };
                if !status.success() {
                    eprintln!("pick enqueue failed for {}", d.display);
                }
            }
            Ok(ExitCode::Ok)
        }
    }
}

fn parse_pick_selection(s: &str) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    for part in s.split(|c: char| c == ',' || c.is_whitespace()) {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        out.push(t.parse::<usize>().context("pick index")?);
    }
    if out.is_empty() {
        bail!("empty pick selection");
    }
    Ok(out)
}

async fn collect_rss(feeds: &[String]) -> (Vec<FlixItem>, usize, usize) {
    let mut items = Vec::new();
    let mut hubs_ok = 0usize;
    let mut hubs_skipped = 0usize;
    for f in feeds {
        let hub = Url::parse(f)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_else(|| "hub".into());
        match fetch_rss_items(f, &hub).await {
            Ok(list) => {
                hubs_ok += 1;
                items.extend(list);
            }
            Err(e) => {
                hubs_skipped += 1;
                eprintln!("rss {f}: {e}");
            }
        }
    }
    (items, hubs_ok, hubs_skipped)
}

async fn cmd_season(
    prefs: &AppPreferences,
    show_or_url: &str,
    auto_download: bool,
    season_filter: Option<u32>,
    show_name: Option<String>,
    opts: &SessionOpts,
) -> Result<ExitCode> {
    let page = if show_or_url.starts_with("http") {
        Url::parse(show_or_url)?
    } else {
        resolve_show_page(show_or_url, season_filter)?
    };
    let scraper = PageScraper::new();
    let html = scraper.fetch_html(&page).await?;
    let mut episodes = enumerate_from_html(&html, &page);
    if let Some(s) = season_filter {
        episodes.retain(|e| e.season == s);
    }
    let show_label = show_name
        .unwrap_or_else(|| show_or_url.to_string());
    AgentEvent::SeasonList {
        show: show_label,
        episodes: episodes
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or_default())
            .collect(),
    }
    .emit();
    if auto_download {
        for ep in episodes {
            if let Some(url) = ep.url {
                // Episode pages → fetch auto
                if MediaDetector::classify(&Url::parse(&url).unwrap_or(page.clone()), None)
                    == MediaClassification::Unknown
                {
                    let _ = cmd_fetch(
                        prefs,
                        &url,
                        false,
                        true,
                        ep.title,
                        opts,
                    )
                    .await;
                } else {
                    let _ = download_url(prefs, &url, ep.title, opts).await;
                }
            }
        }
    }
    Ok(ExitCode::Ok)
}

fn resolve_show_page(name: &str, season: Option<u32>) -> Result<Url> {
    let needle = name.to_lowercase();
    // 1) bookmarks
    let bookmarks = BookmarkStore::load();
    for b in &bookmarks.items {
        if b.title.to_lowercase().contains(&needle) || b.url.to_lowercase().contains(&needle) {
            return Url::parse(&b.url).context("bookmark url");
        }
    }
    // 2) last flix digest
    let last = FlixLastDigest::load();
    for it in &last.items {
        let hay = format!(
            "{} {} {}",
            it.title,
            it.display,
            it.show.clone().unwrap_or_default()
        )
        .to_lowercase();
        if hay.contains(&needle) {
            if let Some(s) = season {
                if it.season == Some(s) {
                    return Url::parse(&it.url).context("digest url");
                }
            } else {
                return Url::parse(&it.url).context("digest url");
            }
        }
    }
    bail!(
        "season by name: no bookmark or flix digest match for '{name}' — pass a page URL or run flix digest"
    );
}

async fn cmd_fetch(
    prefs: &AppPreferences,
    page_url: &str,
    detect_only: bool,
    auto_download: bool,
    title: Option<String>,
    opts: &SessionOpts,
) -> Result<ExitCode> {
    let page = Url::parse(page_url).context("page url")?;
    let (html, page_title) = if opts.via_app {
        match via_app_request(ViaAppRequest::Fetch {
            url: page_url.to_string(),
        })
        .await
        {
            Ok(ViaAppResponse::Html { html, title: t, .. }) => (html, title.or(t)),
            Ok(ViaAppResponse::Pong) | Ok(ViaAppResponse::Opened { .. }) => {
                bail!("unexpected via-app response for fetch")
            }
            Ok(ViaAppResponse::Error { message }) => {
                eprintln!("via-app: {message}; falling back to scrape");
                let scraper = PageScraper::new();
                let html = scraper.fetch_html(&page).await?;
                let t = title.clone().or_else(|| PageScraper::extract_title(&html));
                (html, t)
            }
            Err(e) => {
                eprintln!("via-app unreachable ({e}); falling back to scrape");
                let scraper = PageScraper::new();
                let html = scraper.fetch_html(&page).await?;
                let t = title.clone().or_else(|| PageScraper::extract_title(&html));
                (html, t)
            }
        }
    } else {
        let scraper = PageScraper::new();
        let html = scraper.fetch_html(&page).await?;
        let t = title.clone().or_else(|| PageScraper::extract_title(&html));
        (html, t)
    };
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
        .build()?;
    let candidates = PageScraper::extract_candidates(&html, &page);
    let mut candidates = resolve_player_iframes(&client, candidates, &page).await;
    if let Some(news_id) = extract_news_id(&html, &page) {
        if let Ok(catalog) = fetch_series_catalog(&client, &page, &news_id).await {
            let mut ordered = catalog.episodes;
            ordered.sort_by_key(|e| {
                let ver_rank = match e.version.as_str() {
                    "vf" => 0,
                    "vostfr" => 1,
                    _ => 2,
                };
                (ver_rank, e.episode)
            });
            for ep in ordered.into_iter().take(8) {
                if let Some(embed) = ep.preferred_embed {
                    if let Ok(streams) = resolve_embed_streams(&client, &embed, &page).await {
                        for mut c in streams {
                            c.source =
                                format!("{} E{:02}", ep.version.to_uppercase(), ep.episode);
                            candidates.push(c);
                        }
                    }
                }
            }
        }
    }

    for c in &candidates {
        let kind = format!("{:?}", MediaDetector::classify(&c.url, c.mime_type.as_deref()))
            .to_lowercase();
        AgentEvent::Detected {
            url: c.url.to_string(),
            kind,
            title: page_title.clone(),
        }
        .emit();
    }
    if detect_only || candidates.is_empty() {
        return Ok(if candidates.is_empty() {
            ExitCode::NotFound
        } else {
            ExitCode::Ok
        });
    }
    if auto_download {
        let Some(first) = candidates.iter().find(|c| is_downloadable(c)) else {
            bail!("no downloadable stream found on this page (only pages/iframes)");
        };
        let o = SessionOpts {
            cookie: opts.cookie.clone(),
            // Streams found inside a player iframe expect the iframe as referer.
            referer: opts.referer.clone().or_else(|| {
                Some(first.page_url.as_ref().map_or(page_url.to_string(), |u| u.to_string()))
            }),
            via_app: opts.via_app,
            json: opts.json,
        };
        return download_url(prefs, first.url.as_str(), page_title, &o).await;
    }
    Ok(ExitCode::Ok)
}

fn is_downloadable(c: &NetworkMediaCandidate) -> bool {
    !matches!(
        MediaDetector::classify(&c.url, c.mime_type.as_deref()),
        MediaClassification::Unknown | MediaClassification::Segment
    )
}

/// Many watch pages hold only a player `<iframe>`; the stream URL sits in the
/// iframe's own HTML. Each non-media candidate is fetched once (Referer = page)
/// and replaced by the streams it contains; at most 4 iframes, one level deep.
async fn resolve_player_iframes(
    client: &reqwest::Client,
    candidates: Vec<NetworkMediaCandidate>,
    page: &Url,
) -> Vec<NetworkMediaCandidate> {
    let mut out = Vec::new();
    let mut tried = 0;
    for c in candidates {
        if is_downloadable(&c) || tried >= 4 {
            out.push(c);
            continue;
        }
        tried += 1;
        let html = async {
            client.get(c.url.clone()).header("Referer", page.as_str()).send().await?
                .error_for_status()?.text().await
        }
        .await;
        let streams: Vec<_> = html
            .map(|h| PageScraper::extract_candidates(&h, &c.url))
            .unwrap_or_default()
            .into_iter()
            .filter(is_downloadable)
            .map(|mut s| {
                s.page_url = Some(c.url.clone());
                s
            })
            .collect();
        if streams.is_empty() {
            out.push(c);
        } else {
            // Streams first so --auto-download picks them over the iframe.
            out.splice(0..0, streams);
        }
    }
    out
}

async fn download_url(
    prefs: &AppPreferences,
    url: &str,
    title: Option<String>,
    opts: &SessionOpts,
) -> Result<ExitCode> {
    let media_url = Url::parse(url).context("media url")?;
    let classification = MediaDetector::classify(&media_url, None);
    if matches!(
        classification,
        MediaClassification::Unknown | MediaClassification::Segment
    ) {
        bail!(
            "URL does not look like a downloadable media manifest/file; for a web page use `streamcraft fetch <url> --auto-download`"
        );
    }

    let mut candidate = NetworkMediaCandidate::new(media_url.clone(), "cli");
    if let Some(ref r) = opts.referer {
        candidate.page_url = Url::parse(r).ok();
    }

    let client = reqwest::Client::builder().user_agent(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
    ).build()?;

    let get = |u: Url| {
        let mut req = client.get(u);
        if let Some(ref c) = opts.cookie {
            req = req.header("Cookie", c);
        }
        if let Some(ref r) = opts.referer {
            req = req.header("Referer", r);
        }
        req
    };

    let media = match classification {
        MediaClassification::Progressive => {
            MediaDetector::make_progressive(&candidate, None, title.clone())
        }
        MediaClassification::Hls => {
            let text = get(media_url.clone())
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            if prefs.drm_gate && text.contains("#EXT-X-KEY") {
                AgentEvent::Failed {
                    job_id: None,
                    message: "DRM detected (#EXT-X-KEY) — download refused".into(),
                }
                .emit();
                return Ok(ExitCode::Drm);
            }
            let playlist = HLSParser::parse(&text, &media_url)?;
            MediaDetector::make_hls(&candidate, &playlist, title.clone())
        }
        MediaClassification::Dash => {
            let text = get(media_url.clone())
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            if prefs.drm_gate
                && (text.contains("ContentProtection") || text.to_lowercase().contains("cenc"))
            {
                AgentEvent::Failed {
                    job_id: None,
                    message: "DRM detected (ContentProtection) — download refused".into(),
                }
                .emit();
                return Ok(ExitCode::Drm);
            }
            let manifest = streamcraft_core::DASHParser::parse(&text, &media_url)?;
            let variants = manifest
                .video_representations
                .iter()
                .map(|r| {
                    let mut v = streamcraft_core::MediaVariant::new(
                        r.segments
                            .first()
                            .map(|s| s.url.clone())
                            .unwrap_or(media_url.clone()),
                    );
                    v.bandwidth = r.bandwidth;
                    if let (Some(w), Some(h)) = (r.width, r.height) {
                        v.resolution = Some(streamcraft_core::Resolution { width: w, height: h });
                    }
                    v.codecs = r.codecs.clone();
                    v.dash_representation_id = Some(r.id.clone());
                    v
                })
                .collect();
            MediaDetector::make_dash(
                &candidate,
                variants,
                vec![],
                vec![],
                title.clone(),
                if prefs.drm_gate && manifest.has_content_protection {
                    Some("ContentProtection".into())
                } else {
                    None
                },
            )
        }
        _ => unreachable!(),
    };

    if matches!(media.protection, MediaProtection::Drm { .. }) && prefs.drm_gate {
        AgentEvent::Failed {
            job_id: None,
            message: "DRM refused".into(),
        }
        .emit();
        return Ok(ExitCode::Drm);
    }

    let job_title = title
        .clone()
        .unwrap_or_else(|| media.display_title());
    let now = Utc::now();
    let service = DownloadService::new(prefs.clone());
    let job_id = service
        .enqueue_media(
            media.clone(),
            None,
            None,
            None,
            None,
            JobCredentials {
                cookie_header: opts.cookie.clone(),
                referer: opts.referer.clone(),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    {
        let _ = CliJobStore::update(|store| store.upsert(CliJobRecord {
            id: job_id,
            title: job_title.clone(),
            source_url: url.to_string(),
            state: "downloading".into(),
            progress: 0.0,
            destination: None,
            error_message: None,
            created_at: now,
            updated_at: now,
            cancel_requested: false,
        }));
    }

    AgentEvent::Progress {
        job_id: job_id.to_string(),
        progress: 0.0,
        state: "queued".into(),
    }
    .emit();

    // Cooperative cancel: poll store while running in a select-ish loop via spawn
    let run = service.run_job(job_id);
    tokio::pin!(run);
    let result = loop {
        if CliJobStore::is_cancel_requested(job_id) {
            let _ = service.queue.cancel(job_id).await;
        }
        tokio::select! {
            r = &mut run => break r,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if CliJobStore::is_cancel_requested(job_id) {
                    let _ = service.queue.cancel(job_id).await;
                }
            }
        }
    };

    match result {
        Ok(path) => {
            let _ = CliJobStore::update(|store| {
                if let Some(j) = store.get_mut(job_id) {
                    j.state = "completed".into();
                    j.progress = 1.0;
                    j.destination = Some(path.display().to_string());
                    j.updated_at = Utc::now();
                }
            });
            AgentEvent::Completed {
                job_id: job_id.to_string(),
                path: path.display().to_string(),
            }
            .emit();
            Ok(ExitCode::Ok)
        }
        Err(e) => {
            let cancelled = CliJobStore::is_cancel_requested(job_id) || e.contains("cancel");
            let drm = e.contains("DRM");
            let _ = CliJobStore::update(|store| {
                if let Some(j) = store.get_mut(job_id) {
                    j.state = if cancelled { "cancelled" } else { "failed" }.into();
                    j.error_message = Some(e.clone());
                    j.updated_at = Utc::now();
                }
            });
            AgentEvent::Failed {
                job_id: Some(job_id.to_string()),
                message: e,
            }
            .emit();
            Ok(if cancelled {
                ExitCode::Cancelled
            } else if drm {
                // Refusal found at manifest time (e.g. DASH ContentProtection).
                ExitCode::Drm
            } else {
                ExitCode::Network
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A page that only embeds a player iframe: fetch must open the iframe and
    /// surface its stream, with the iframe as the stream's page (referer).
    #[tokio::test]
    async fn player_iframe_stream_is_surfaced() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(req.to_ascii_lowercase().contains("referer: https://watch.example/film/1"));
            let body = "<script>var f='https://cdn.example/files/aa/x.m3u8';</script>";
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let page = Url::parse("https://watch.example/film/1").unwrap();
        let iframe = Url::parse(&format!("http://127.0.0.1:{port}/iframe/abc")).unwrap();
        let mut c = NetworkMediaCandidate::new(iframe.clone(), "scrape");
        c.page_url = Some(page.clone());
        assert!(!is_downloadable(&c));

        let out = resolve_player_iframes(&reqwest::Client::new(), vec![c], &page).await;
        assert_eq!(out[0].url.as_str(), "https://cdn.example/files/aa/x.m3u8");
        assert_eq!(out[0].page_url.as_ref(), Some(&iframe));
        assert!(is_downloadable(&out[0]));
    }
}
