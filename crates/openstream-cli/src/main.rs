use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use openstream_core::agent::events::{AgentEvent, ExitCode};
use openstream_core::agent::flix::{fetch_rss_items, FlixSeenStore};
use openstream_core::agent::catalog::{extract_news_id, fetch_series_catalog, resolve_embed_streams};
use openstream_core::agent::scrape::PageScraper;
use openstream_core::agent::season::enumerate_from_html;
use openstream_core::agent::via_app::{request as via_app_request, ViaAppRequest, ViaAppResponse};
use openstream_core::download::service::DownloadService;
use openstream_core::media::detector::MediaDetector;
use openstream_core::media::hls::HLSParser;
use openstream_core::media::models::{MediaClassification, MediaProtection, NetworkMediaCandidate};
use openstream_core::store::prefs::AppPreferences;
use openstream_ffmpeg::FFmpegPaths;
use url::Url;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "openstream-cli", version, about = "StreamCraft headless CLI (JSON Lines)")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    #[arg(long, global = true)]
    cookies: Option<PathBuf>,

    #[arg(long, global = true, help = "Forward page fetch to running GUI app socket")]
    via_app: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Download a known media URL (.m3u8 / .mpd / .mp4)
    Download {
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Fetch a page and detect media (scrape; use --via-app for JS pages)
    Fetch {
        page_url: String,
        #[arg(long)]
        detect_only: bool,
        #[arg(long)]
        auto_download: bool,
        #[arg(long)]
        title: Option<String>,
    },
    /// List / download season episodes
    Season {
        show_or_url: String,
        #[arg(long)]
        auto_download: bool,
    },
    /// Flix digest over favorite hub RSS
    Flix {
        #[command(subcommand)]
        cmd: FlixCmd,
    },
    Status,
    Wait {
        job_id: Option<String>,
        #[arg(long, default_value_t = 3600)]
        timeout: u64,
    },
    Cancel {
        job_id: String,
    },
    /// Check ffmpeg sidecar
    Doctor,
}

#[derive(Subcommand, Debug)]
enum FlixCmd {
    Digest {
        #[arg(long)]
        feed: Vec<String>,
        #[arg(long)]
        mark_seen: bool,
    },
    Scan {
        #[arg(long)]
        feed: Vec<String>,
    },
    Seen,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let code = match run(cli).await {
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

async fn run(cli: Cli) -> Result<ExitCode> {
    let mut prefs = AppPreferences::load();
    match cli.command {
        Commands::Doctor => {
            match FFmpegPaths::resolve() {
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
                        message: "ffmpeg not found".into(),
                    }
                    .emit();
                    Ok(ExitCode::Ffmpeg)
                }
            }
        }
        Commands::Download { url, title, out } => {
            if let Some(dir) = out {
                prefs.export_dir = dir;
            }
            download_url(&prefs, &url, title).await
        }
        Commands::Fetch {
            page_url,
            detect_only,
            auto_download,
            title,
        } => {
            let page = Url::parse(&page_url).context("page url")?;
            let (html, page_title) = if cli.via_app {
                match via_app_request(ViaAppRequest::Fetch {
                    url: page_url.clone(),
                })
                .await
                {
                    Ok(ViaAppResponse::Html { html, title: t, .. }) => {
                        (html, title.or(t))
                    }
                    Ok(ViaAppResponse::Pong) => {
                        bail!("unexpected pong from via-app");
                    }
                    Ok(ViaAppResponse::Error { message }) => {
                        AgentEvent::Log {
                            level: "warn".into(),
                            message: format!("via-app: {message}; falling back to scrape"),
                        }
                        .emit();
                        let scraper = PageScraper::new();
                        let html = scraper.fetch_html(&page).await?;
                        let t = title.clone().or_else(|| PageScraper::extract_title(&html));
                        (html, t)
                    }
                    Err(e) => {
                        AgentEvent::Log {
                            level: "warn".into(),
                            message: format!("via-app unreachable ({e}); falling back to scrape"),
                        }
                        .emit();
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
            let mut candidates = PageScraper::extract_candidates(&html, &page);
            let client = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
                .build()?;
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
                                    c.source = format!("{} E{:02}", ep.version.to_uppercase(), ep.episode);
                                    candidates.push(c);
                                }
                            }
                        }
                    }
                }
            }

            for c in &candidates {
                let kind = format!("{:?}", MediaDetector::classify(&c.url, c.mime_type.as_deref())).to_lowercase();
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
                let first = &candidates[0];
                return download_url(&prefs, first.url.as_str(), page_title).await;
            }
            Ok(ExitCode::Ok)
        }
        Commands::Season {
            show_or_url,
            auto_download,
        } => {
            let page = if show_or_url.starts_with("http") {
                Url::parse(&show_or_url)?
            } else {
                bail!("season by name requires favorites/search (Phase R4); pass a page URL for now");
            };
            let scraper = PageScraper::new();
            let html = scraper.fetch_html(&page).await?;
            let episodes = enumerate_from_html(&html, &page);
            AgentEvent::SeasonList {
                show: show_or_url.clone(),
                episodes: episodes
                    .iter()
                    .map(|e| serde_json::to_value(e).unwrap_or_default())
                    .collect(),
            }
            .emit();
            if auto_download {
                for ep in episodes {
                    if let Some(url) = ep.url {
                        let _ = download_url(&prefs, &url, ep.title).await;
                    }
                }
            }
            Ok(ExitCode::Ok)
        }
        Commands::Flix { cmd } => {
            let seen_path = dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("StreamCraft")
                .join("flix_seen.json");
            match cmd {
                FlixCmd::Seen => {
                    let seen = FlixSeenStore::load(&seen_path);
                    AgentEvent::Log {
                        level: "info".into(),
                        message: format!("seen={}", seen.seen_ids.len()),
                    }
                    .emit();
                    Ok(ExitCode::Ok)
                }
                FlixCmd::Digest { feed, mark_seen } => {
                    run_flix_feeds(feed, mark_seen, &seen_path).await
                }
                FlixCmd::Scan { feed } => run_flix_feeds(feed, false, &seen_path).await,
            }
        }
        Commands::Status => {
            AgentEvent::Status { jobs: vec![] }.emit();
            Ok(ExitCode::Ok)
        }
        Commands::Wait { job_id, timeout } => {
            let _ = (job_id, timeout);
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(ExitCode::Ok)
        }
        Commands::Cancel { job_id } => {
            let _ = Uuid::parse_str(&job_id);
            AgentEvent::Log {
                level: "info".into(),
                message: format!("cancel requested for {job_id}"),
            }
            .emit();
            Ok(ExitCode::Ok)
        }
    }
}


async fn run_flix_feeds(feed: Vec<String>, mark_seen: bool, seen_path: &PathBuf) -> Result<ExitCode> {
    let mut seen = FlixSeenStore::load(seen_path);
    let mut items = Vec::new();
    for f in feed {
        let hub = Url::parse(&f)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_else(|| "hub".into());
        match fetch_rss_items(&f, &hub).await {
            Ok(list) => {
                for it in list {
                    if !seen.contains(&it.id) {
                        items.push(it);
                    }
                }
            }
            Err(e) => AgentEvent::Log {
                level: "warn".into(),
                message: format!("rss {f}: {e}"),
            }
            .emit(),
        }
    }
    if mark_seen {
        for it in &items {
            seen.mark(&it.id);
        }
        let _ = seen.save(seen_path);
    }
    AgentEvent::FlixDigest {
        count: items.len(),
        items: items
            .iter()
            .map(|i| serde_json::to_value(i).unwrap_or_default())
            .collect(),
    }
    .emit();
    Ok(ExitCode::Ok)
}

async fn download_url(prefs: &AppPreferences, url: &str, title: Option<String>) -> Result<ExitCode> {
    let media_url = Url::parse(url).context("media url")?;
    let classification = MediaDetector::classify(&media_url, None);
    if matches!(classification, MediaClassification::Unknown | MediaClassification::Segment) {
        bail!("URL does not look like a downloadable media manifest/file");
    }

    let candidate = NetworkMediaCandidate::new(media_url.clone(), "cli");
    let client = reqwest::Client::new();

    let media = match classification {
        MediaClassification::Progressive => {
            MediaDetector::make_progressive(&candidate, None, title)
        }
        MediaClassification::Hls => {
            let text = client
                .get(media_url.clone())
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            // DRM gate
            if prefs.drm_gate && text.contains("#EXT-X-KEY") {
                AgentEvent::Failed {
                    job_id: None,
                    message: "DRM detected (#EXT-X-KEY) — download refused".into(),
                }
                .emit();
                return Ok(ExitCode::Drm);
            }
            let playlist = HLSParser::parse(&text, &media_url)?;
            MediaDetector::make_hls(&candidate, &playlist, title)
        }
        MediaClassification::Dash => {
            let text = client
                .get(media_url.clone())
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
            let manifest = openstream_core::DASHParser::parse(&text, &media_url)?;
            let variants = manifest
                .video_representations
                .iter()
                .map(|r| {
                    let mut v = openstream_core::MediaVariant::new(
                        r.segments
                            .first()
                            .map(|s| s.url.clone())
                            .unwrap_or(media_url.clone()),
                    );
                    v.bandwidth = r.bandwidth;
                    if let (Some(w), Some(h)) = (r.width, r.height) {
                        v.resolution = Some(openstream_core::Resolution { width: w, height: h });
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
                title,
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

    let service = DownloadService::new(prefs.clone());
    let job_id = service.enqueue_media(media, None, None, None, None).await.map_err(|e| anyhow::anyhow!(e))?;
    AgentEvent::Progress {
        job_id: job_id.to_string(),
        progress: 0.0,
        state: "queued".into(),
    }
    .emit();

    match service.run_job(job_id).await {
        Ok(path) => {
            AgentEvent::Completed {
                job_id: job_id.to_string(),
                path: path.display().to_string(),
            }
            .emit();
            Ok(ExitCode::Ok)
        }
        Err(e) => {
            AgentEvent::Failed {
                job_id: Some(job_id.to_string()),
                message: e,
            }
            .emit();
            Ok(ExitCode::Network)
        }
    }
}
