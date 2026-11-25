//! Browser/network candidate → DetectedMedia (port of Tauri resolve_hls / resolve_dash).
//! Called from `app.rs` (ingest + direct URL). No prior dedicated module — was inline `classify()`.

use streamcraft_core::media::dash::DASHParser;
use streamcraft_core::media::detector::MediaDetector;
use streamcraft_core::media::heuristics::MediaURLHeuristics;
use streamcraft_core::media::hls::{HLSParser, HLSPlaylistKind};
use streamcraft_core::media::models::{
    DetectedMedia, ManifestKind, MediaClassification, MediaProtection, MediaVariant,
    NetworkMediaCandidate, Resolution,
};
use url::Url;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

pub fn http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder().user_agent(UA).build().ok()
}

#[derive(Clone)]
pub struct IngestInput {
    pub url: String,
    pub mime: Option<String>,
    pub source: Option<String>,
    pub page: Option<String>,
    pub page_title: Option<String>,
    pub fallback_page_url: Option<String>,
    pub drm_gate: bool,
}

/// Classify + resolve. Returns None for segments/unknown/non-media.
pub async fn ingest_candidate(input: IngestInput) -> Option<DetectedMedia> {
    let url = Url::parse(&input.url).ok()?;
    if !MediaURLHeuristics::looks_like_media(&url, input.mime.as_deref()) {
        return None;
    }
    let class = MediaDetector::classify(&url, input.mime.as_deref());

    let page_url = input
        .page
        .as_deref()
        .and_then(|p| Url::parse(p).ok())
        .or_else(|| {
            input
                .fallback_page_url
                .as_deref()
                .and_then(|p| Url::parse(p).ok())
        });

    // Iframe player page (vidzy embed, …): scrape for real m3u8.
    if matches!(
        class,
        MediaClassification::Unknown | MediaClassification::Progressive
    ) && MediaURLHeuristics::looks_like_embed_player(&url)
    {
        let client = http_client()?;
        let referer = page_url.clone().unwrap_or_else(|| url.clone());
        if let Ok(cands) =
            streamcraft_core::agent::catalog::resolve_embed_streams(&client, &url, &referer).await
        {
            for c in cands {
                let mut nested = input.clone();
                nested.url = c.url.to_string();
                nested.mime = c
                    .mime_type
                    .clone()
                    .or_else(|| Some("application/vnd.apple.mpegurl".into()));
                nested.source = Some(format!(
                    "{}>embed",
                    input.source.clone().unwrap_or_else(|| "iframe".into())
                ));
                nested.page = Some(url.as_str().to_string());
                if let Some(media) = Box::pin(ingest_candidate(nested)).await {
                    return Some(media);
                }
            }
        }
        // Fall through — hooks may still surface the m3u8 after play.
        if matches!(class, MediaClassification::Unknown) {
            return None;
        }
    }

    if matches!(
        class,
        MediaClassification::Segment | MediaClassification::Unknown
    ) {
        return None;
    }

    let mut candidate = NetworkMediaCandidate::new(
        url.clone(),
        input
            .source
            .clone()
            .unwrap_or_else(|| "resourceTiming".into()),
    );
    candidate.mime_type = input.mime.clone();
    candidate.page_url = page_url.clone();

    let title = Some(streamcraft_core::media::page_title::resolve_title(
        &[],
        input.page_title.as_deref(),
        page_url.as_ref(),
        &url,
    ));

    let client = http_client();
    let mut media = match class {
        MediaClassification::Progressive => {
            MediaDetector::make_progressive(&candidate, None, title)
        }
        MediaClassification::Hls => {
            if let Some(client) = client.as_ref() {
                match resolve_hls(client, &candidate, title.clone(), input.drm_gate).await {
                    Ok(m) => m,
                    Err(_) => MediaDetector::make_hls_url(&candidate, title),
                }
            } else {
                MediaDetector::make_hls_url(&candidate, title)
            }
        }
        MediaClassification::Dash => {
            let client = client.as_ref()?;
            resolve_dash(client, &candidate, title, input.drm_gate)
                .await
                .ok()?
        }
        MediaClassification::Segment | MediaClassification::Unknown => return None,
    };

    if media.page_url.is_none() {
        media.page_url = page_url;
    }
    Some(media)
}

pub async fn resolve_direct_url(raw: &str, drm_gate: bool) -> Result<DetectedMedia, String> {
    let url = Url::parse(raw).map_err(|e| e.to_string())?;
    if !MediaURLHeuristics::looks_like_media(&url, None) {
        return Err("Not a media URL".into());
    }
    let class = MediaDetector::classify(&url, None);
    let candidate = NetworkMediaCandidate::new(url.clone(), "ui");
    let client = http_client().ok_or_else(|| "HTTP client failed".to_string())?;
    match class {
        MediaClassification::Progressive => {
            Ok(MediaDetector::make_progressive(&candidate, None, None))
        }
        MediaClassification::Hls => resolve_hls(&client, &candidate, None, drm_gate).await,
        MediaClassification::Dash => resolve_dash(&client, &candidate, None, drm_gate).await,
        MediaClassification::Segment | MediaClassification::Unknown => {
            Err("URL looks like a segment, not a playlist".into())
        }
    }
}

pub async fn resolve_hls(
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
        media.protection = MediaProtection::Drm {
            reason: "#EXT-X-KEY".into(),
        };
        return Ok(media);
    }

    let playlist = HLSParser::parse(&text, &candidate.url).map_err(|e| e.to_string())?;
    let mut media = MediaDetector::make_hls(candidate, &playlist, title);
    if matches!(playlist.kind, HLSPlaylistKind::Master) {
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

pub async fn resolve_dash(
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

pub fn kind_label(kind: ManifestKind) -> &'static str {
    match kind {
        ManifestKind::Hls => "HLS",
        ManifestKind::Dash => "DASH",
        ManifestKind::Progressive => "MP4",
    }
}
