use std::path::PathBuf;

use thiserror::Error;
use url::Url;

use crate::media::hls::{HLSParser, HLSPlaylistKind};
use crate::media::models::{ManifestKind, MediaVariant};

#[derive(Debug, Error)]
pub enum PlanError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Hls(#[from] crate::media::hls::HLSParserError),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone)]
pub struct DownloadUnit {
    pub index: usize,
    pub url: Url,
    pub file_name: String,
    pub duration: Option<f64>,
    pub is_init: bool,
}

#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub units: Vec<DownloadUnit>,
    pub kind: ManifestKind,
    pub local_playlist_hint: Option<PathBuf>,
}

pub async fn plan_for_variant(
    client: &reqwest::Client,
    variant: &MediaVariant,
    kind: ManifestKind,
) -> Result<DownloadPlan, PlanError> {
    match kind {
        ManifestKind::Progressive => Ok(DownloadPlan {
            units: vec![DownloadUnit {
                index: 0,
                url: variant.playlist_url.clone(),
                file_name: "media.mp4".into(),
                duration: None,
                is_init: false,
            }],
            kind,
            local_playlist_hint: None,
        }),
        ManifestKind::Hls => plan_hls(client, &variant.playlist_url).await,
        ManifestKind::Dash => Err(PlanError::Message(
            "DASH planning requires pre-resolved segments; use plan_dash_units".into(),
        )),
    }
}

pub fn plan_dash_units(units: Vec<DownloadUnit>) -> DownloadPlan {
    DownloadPlan {
        units,
        kind: ManifestKind::Dash,
        local_playlist_hint: None,
    }
}

async fn plan_hls(client: &reqwest::Client, url: &Url) -> Result<DownloadPlan, PlanError> {
    let text = client
        .get(url.clone())
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
        )
        .header("Referer", url.as_str())
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let playlist = HLSParser::parse(&text, url)?;
    match playlist.kind {
        HLSPlaylistKind::Master => {
            // Prefer highest bandwidth that is not clearly audio-only
            let best = playlist
                .streams
                .iter()
                .filter(|s| {
                    let c = s.codecs.as_deref().unwrap_or("").to_lowercase();
                    !(c.starts_with("mp4a") && !c.contains("avc") && !c.contains("hvc") && !c.contains("hev"))
                        || s.resolution.is_some()
                })
                .max_by_key(|s| s.bandwidth.unwrap_or(0))
                .or_else(|| {
                    playlist
                        .streams
                        .iter()
                        .max_by_key(|s| s.bandwidth.unwrap_or(0))
                })
                .ok_or_else(|| PlanError::Message("empty HLS master".into()))?;
            // Skip audio-only playlists named index-a*
            let uri_s = best.uri.as_str().to_lowercase();
            if uri_s.contains("index-a") && !uri_s.contains("index-v") {
                let video = playlist
                    .streams
                    .iter()
                    .filter(|s| {
                        let u = s.uri.as_str().to_lowercase();
                        !u.contains("index-a") || u.contains("index-v")
                    })
                    .max_by_key(|s| s.bandwidth.unwrap_or(0));
                if let Some(v) = video {
                    return Box::pin(plan_hls(client, &v.uri)).await;
                }
            }
            Box::pin(plan_hls(client, &best.uri)).await
        }
        HLSPlaylistKind::Media => {
            let media = playlist
                .media
                .ok_or_else(|| PlanError::Message("media playlist missing body".into()))?;
            let mut units = Vec::new();
            let mut index = 0usize;
            if let Some(map) = media.map_uri {
                units.push(DownloadUnit {
                    index,
                    url: map,
                    file_name: format!("{index:05}_init.mp4"),
                    duration: None,
                    is_init: true,
                });
                index += 1;
            }
            for seg in media.segments {
                let ext = seg
                    .uri
                    .path_segments()
                    .and_then(|mut s| s.next_back())
                    .and_then(|n| n.rsplit_once('.'))
                    .map(|(_, e)| e.to_lowercase())
                    .unwrap_or_else(|| "ts".into());
                // CDN sometimes uses .image lure → plan as .ts for mux
                let file_ext = if ext == "image" || ext == "png" || ext.is_empty() {
                    "ts".into()
                } else {
                    ext
                };
                units.push(DownloadUnit {
                    index,
                    url: seg.uri.clone(),
                    file_name: format!("{index:05}_seg.{file_ext}"),
                    duration: Some(seg.duration),
                    is_init: false,
                });
                index += 1;
            }
            Ok(DownloadPlan {
                units,
                kind: ManifestKind::Hls,
                local_playlist_hint: None,
            })
        }
    }
}


pub fn prefix_units(plan: DownloadPlan, prefix: &str) -> DownloadPlan {
    DownloadPlan {
        units: plan
            .units
            .into_iter()
            .map(|mut u| {
                u.file_name = format!("{prefix}{}", u.file_name);
                u
            })
            .collect(),
        kind: plan.kind,
        local_playlist_hint: plan.local_playlist_hint,
    }
}
