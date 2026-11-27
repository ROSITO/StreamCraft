//! OpenStream core library — media detection, parsers, download, stores, agent helpers.

pub mod agent;
pub mod assemble;
pub mod download;
pub mod media;
pub mod session;
pub mod store;

pub use media::models::{
    DetectedMedia, DownloadState, ManifestKind, MediaClassification, MediaProtection, MediaTrack,
    MediaTrackKind, MediaVariant, NetworkMediaCandidate, Resolution,
};
pub use media::detector::MediaDetector;
pub use media::heuristics::MediaURLHeuristics;
pub use media::hls::{HLSParser, HLSPlaylist};
pub use media::dash::{DASHManifest, DASHParser};
pub use media::export_naming::{ExportMetadata, ExportNaming, ExportNamingContext, ExportNamingPreset};
pub use store::prefs::{AppPreferences, ProxyMode, VpnIndicatorFilter, default_export_dir};
pub use agent::catalog::{extract_news_id, fetch_series_catalog, resolve_embed_streams, SeriesCatalog};
pub use agent::popup_guard::PopupGuard;
pub use agent::events::{AgentEvent, ExitCode};
pub use download::queue::DownloadQueue;
pub use download::service::{DownloadService, JobCredentials};
