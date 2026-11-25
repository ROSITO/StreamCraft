use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestKind {
    Progressive,
    Hls,
    Dash,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaProtection {
    None,
    Drm { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaClassification {
    Progressive,
    Hls,
    Dash,
    Segment,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaTrackKind {
    Audio,
    Subtitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Downloading,
    Assembling,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaTrack {
    pub id: Uuid,
    pub kind: MediaTrackKind,
    pub language: Option<String>,
    pub name: Option<String>,
    pub url: Url,
    pub dash_representation_id: Option<String>,
    pub is_default: bool,
    pub codecs: Option<String>,
}

impl MediaTrack {
    pub fn preferred_audio(tracks: &[MediaTrack]) -> Option<&MediaTrack> {
        if tracks.is_empty() {
            return None;
        }
        let french = ["fr", "fra", "fre", "fr-fr", "fr_fr"];
        tracks
            .iter()
            .find(|t| {
                let lang = t
                    .language
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .replace('_', "-");
                let name = t.name.as_deref().unwrap_or("").to_lowercase();
                french.iter().any(|c| *c == lang)
                    || lang.starts_with("fr-")
                    || name.contains("français")
                    || name.contains("francais")
                    || name == "french"
                    || name.contains(" french")
                    || name.starts_with("fr ")
                    || name == "fr"
                    || name.starts_with("fra")
            })
            .or_else(|| tracks.iter().find(|t| t.is_default))
            .or_else(|| tracks.first())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaVariant {
    pub id: Uuid,
    pub bandwidth: Option<i64>,
    pub resolution: Option<Resolution>,
    pub codecs: Option<String>,
    pub playlist_url: Url,
    pub average_bandwidth: Option<i64>,
    pub dash_representation_id: Option<String>,
}

impl MediaVariant {
    pub fn new(playlist_url: Url) -> Self {
        Self {
            id: Uuid::new_v4(),
            bandwidth: None,
            resolution: None,
            codecs: None,
            playlist_url,
            average_bandwidth: None,
            dash_representation_id: None,
        }
    }

    pub fn display_label(&self) -> String {
        let file = self
            .playlist_url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("playlist")
            .to_string();
        let mut parts = vec![file];
        if let Some(r) = self.resolution {
            if r.width > 0 && r.height > 0 {
                parts.push(format!("{}×{}", r.width, r.height));
            }
        }
        if let Some(bw) = self.bandwidth {
            parts.push(format_bandwidth(bw));
        }
        parts.join(" · ")
    }

    pub fn format_duration(seconds: Option<f64>) -> Option<String> {
        let seconds = seconds.filter(|s| s.is_finite() && *s > 0.0)?;
        let total = seconds.round() as i64;
        let h = total / 3600;
        let m = (total % 3600) / 60;
        let s = total % 60;
        if h > 0 {
            Some(format!("{h}h {m:02}m"))
        } else if m > 0 {
            Some(format!("{m}m {s:02}s"))
        } else {
            Some(format!("{s}s"))
        }
    }
}

fn format_bandwidth(bps: i64) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} Mbps", bps as f64 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{} kbps", bps / 1_000)
    } else {
        format!("{bps} bps")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedMedia {
    pub id: Uuid,
    pub source_url: Url,
    pub kind: ManifestKind,
    pub protection: MediaProtection,
    pub suggested_title: Option<String>,
    pub page_url: Option<Url>,
    pub variants: Vec<MediaVariant>,
    pub preferred_variant_id: Option<Uuid>,
    pub audio_tracks: Vec<MediaTrack>,
    pub subtitle_tracks: Vec<MediaTrack>,
    pub is_vod: bool,
    pub segment_count: Option<usize>,
    pub duration_seconds: Option<f64>,
    pub content_length: Option<i64>,
    pub observed_at: DateTime<Utc>,
}

impl DetectedMedia {
    pub fn preferred_variant(&self) -> Option<&MediaVariant> {
        if let Some(id) = self.preferred_variant_id {
            if let Some(v) = self.variants.iter().find(|v| v.id == id) {
                return Some(v);
            }
        }
        self.variants.iter().max_by_key(|v| v.bandwidth.unwrap_or(0))
    }

    pub fn display_title(&self) -> String {
        self.suggested_title.clone().unwrap_or_else(|| {
            self.source_url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .unwrap_or("video")
                .to_string()
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMediaCandidate {
    pub url: Url,
    pub mime_type: Option<String>,
    pub page_url: Option<Url>,
    pub source: String,
    pub observed_at: DateTime<Utc>,
}

impl NetworkMediaCandidate {
    pub fn new(url: Url, source: impl Into<String>) -> Self {
        Self {
            url,
            mime_type: None,
            page_url: None,
            source: source.into(),
            observed_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadJob {
    pub id: Uuid,
    pub media: DetectedMedia,
    pub selected_variant: MediaVariant,
    pub selected_audio: Option<MediaTrack>,
    pub selected_subtitle: Option<MediaTrack>,
    pub state: DownloadState,
    pub destination: Option<std::path::PathBuf>,
    pub progress: f64,
    pub error_message: Option<String>,
    pub export_metadata: Option<crate::media::export_naming::ExportMetadata>,
    #[serde(default)]
    pub cookie_header: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
    /// When true, skip segment download and remux existing `.parts/{id}`.
    #[serde(default)]
    pub assemble_only: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
