use uuid::Uuid;

use super::cdn_gate::suggest_urlset_audio_siblings;
use super::hls::{HLSPlaylist, HLSPlaylistKind, HLSRenditionKind};
use super::models::{
    DetectedMedia, ManifestKind, MediaClassification, MediaProtection, MediaTrack, MediaTrackKind,
    MediaVariant, NetworkMediaCandidate,
};

pub struct MediaDetector;

impl MediaDetector {
    pub fn classify(url: &url::Url, mime_type: Option<&str>) -> MediaClassification {
        let ext = url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .and_then(|n| n.rsplit_once('.'))
            .map(|(_, e)| e.to_lowercase())
            .unwrap_or_default();
        let mime = mime_type.unwrap_or("").to_lowercase();
        let haystack = url.as_str().to_lowercase();

        if mime.contains("mpegurl")
            || mime.contains("application/vnd.apple.mpegurl")
            || ext == "m3u8"
            || haystack.contains(".m3u8")
        {
            return MediaClassification::Hls;
        }
        if mime.contains("dash+xml") || ext == "mpd" || haystack.contains(".mpd") {
            return MediaClassification::Dash;
        }
        if ["mp4", "m4v", "mov", "webm"].contains(&ext.as_str())
            || mime.starts_with("video/")
            || mime.contains("application/mp4")
        {
            if ext == "ts" || ext == "m4s" {
                return MediaClassification::Segment;
            }
            return MediaClassification::Progressive;
        }
        if ext == "ts" || ext == "m4s" || mime.contains("mp2t") {
            return MediaClassification::Segment;
        }
        MediaClassification::Unknown
    }

    pub fn make_progressive(
        candidate: &NetworkMediaCandidate,
        content_length: Option<i64>,
        title: Option<String>,
    ) -> DetectedMedia {
        let variant = MediaVariant::new(candidate.url.clone());
        let preferred = variant.id;
        DetectedMedia {
            id: Uuid::new_v4(),
            source_url: candidate.url.clone(),
            kind: ManifestKind::Progressive,
            protection: MediaProtection::None,
            suggested_title: title.or_else(|| {
                candidate
                    .url
                    .path_segments()
                    .and_then(|mut s| s.next_back())
                    .map(|s| s.to_string())
            }),
            page_url: candidate.page_url.clone(),
            variants: vec![variant],
            preferred_variant_id: Some(preferred),
            audio_tracks: vec![],
            subtitle_tracks: vec![],
            is_vod: true,
            segment_count: Some(1),
            duration_seconds: None,
            content_length,
            observed_at: candidate.observed_at,
        }
    }

    pub fn make_hls(
        candidate: &NetworkMediaCandidate,
        playlist: &HLSPlaylist,
        title: Option<String>,
    ) -> DetectedMedia {
        let audio_tracks: Vec<MediaTrack> = playlist
            .renditions
            .iter()
            .filter(|r| r.kind == HLSRenditionKind::Audio)
            .filter_map(|r| {
                Some(MediaTrack {
                    id: Uuid::new_v4(),
                    kind: MediaTrackKind::Audio,
                    language: r.language.clone(),
                    name: r.name.clone(),
                    url: r.uri.clone()?,
                    dash_representation_id: None,
                    is_default: r.is_default,
                    codecs: None,
                })
            })
            .collect();
        let subtitle_tracks: Vec<MediaTrack> = playlist
            .renditions
            .iter()
            .filter(|r| {
                matches!(
                    r.kind,
                    HLSRenditionKind::Subtitles | HLSRenditionKind::ClosedCaptions
                )
            })
            .filter_map(|r| {
                Some(MediaTrack {
                    id: Uuid::new_v4(),
                    kind: MediaTrackKind::Subtitle,
                    language: r.language.clone(),
                    name: r.name.clone(),
                    url: r.uri.clone()?,
                    dash_representation_id: None,
                    is_default: r.is_default,
                    codecs: None,
                })
            })
            .collect();

        match playlist.kind {
            HLSPlaylistKind::Master => {
                let variants: Vec<MediaVariant> = playlist
                    .streams
                    .iter()
                    .map(|s| MediaVariant {
                        id: Uuid::new_v4(),
                        bandwidth: s.bandwidth,
                        resolution: s.resolution,
                        codecs: s.codecs.clone(),
                        playlist_url: s.uri.clone(),
                        average_bandwidth: s.average_bandwidth,
                        dash_representation_id: None,
                    })
                    .collect();
                let preferred = variants
                    .iter()
                    .max_by_key(|v| v.bandwidth.unwrap_or(0))
                    .map(|v| v.id);
                DetectedMedia {
                    id: Uuid::new_v4(),
                    source_url: candidate.url.clone(),
                    kind: ManifestKind::Hls,
                    protection: MediaProtection::None,
                    suggested_title: title,
                    page_url: candidate.page_url.clone(),
                    variants,
                    preferred_variant_id: preferred,
                    audio_tracks,
                    subtitle_tracks,
                    is_vod: true,
                    segment_count: None,
                    duration_seconds: None,
                    content_length: None,
                    observed_at: candidate.observed_at,
                }
            }
            HLSPlaylistKind::Media => {
                let variant = MediaVariant::new(candidate.url.clone());
                let preferred = variant.id;
                let segment_count = playlist.media.as_ref().map(|m| m.segments.len());
                let duration_seconds = playlist.media.as_ref().map(|m| m.total_duration());
                // Demuxed CDN video (index-v*) has no EXT-X-MEDIA — invent FR/EN siblings.
                let audio_tracks = if audio_tracks.is_empty() {
                    suggest_urlset_audio_siblings(&candidate.url)
                        .into_iter()
                        .map(|(url, lang, name, is_default)| MediaTrack {
                            id: Uuid::new_v4(),
                            kind: MediaTrackKind::Audio,
                            language: Some(lang.into()),
                            name: Some(name.into()),
                            url,
                            dash_representation_id: None,
                            is_default,
                            codecs: None,
                        })
                        .collect()
                } else {
                    audio_tracks
                };
                DetectedMedia {
                    id: Uuid::new_v4(),
                    source_url: candidate.url.clone(),
                    kind: ManifestKind::Hls,
                    protection: MediaProtection::None,
                    suggested_title: title,
                    page_url: candidate.page_url.clone(),
                    variants: vec![variant],
                    preferred_variant_id: Some(preferred),
                    audio_tracks,
                    subtitle_tracks,
                    is_vod: playlist
                        .media
                        .as_ref()
                        .map(|m| m.has_end_list)
                        .unwrap_or(false),
                    segment_count,
                    duration_seconds,
                    content_length: None,
                    observed_at: candidate.observed_at,
                }
            }
        }
    }

    pub fn make_dash(
        candidate: &NetworkMediaCandidate,
        variants: Vec<MediaVariant>,
        audio_tracks: Vec<MediaTrack>,
        subtitle_tracks: Vec<MediaTrack>,
        title: Option<String>,
        has_drm: Option<String>,
    ) -> DetectedMedia {
        let preferred = variants
            .iter()
            .max_by_key(|v| v.bandwidth.unwrap_or(0))
            .map(|v| v.id);
        let protection = match has_drm {
            Some(reason) => MediaProtection::Drm { reason },
            None => MediaProtection::None,
        };
        DetectedMedia {
            id: Uuid::new_v4(),
            source_url: candidate.url.clone(),
            kind: ManifestKind::Dash,
            protection,
            suggested_title: title,
            page_url: candidate.page_url.clone(),
            variants,
            preferred_variant_id: preferred,
            audio_tracks,
            subtitle_tracks,
            is_vod: true,
            segment_count: None,
            duration_seconds: None,
            content_length: None,
            observed_at: candidate.observed_at,
        }
    }
    /// Build an HLS DetectedMedia from a media playlist / master URL without parsing body.
    pub fn make_hls_url(candidate: &NetworkMediaCandidate, title: Option<String>) -> DetectedMedia {
        let variant = MediaVariant::new(candidate.url.clone());
        let preferred = variant.id;
        DetectedMedia {
            id: Uuid::new_v4(),
            source_url: candidate.url.clone(),
            kind: ManifestKind::Hls,
            protection: MediaProtection::None,
            suggested_title: title.or_else(|| {
                candidate
                    .url
                    .path_segments()
                    .and_then(|mut s| s.next_back())
                    .map(|s| s.to_string())
            }),
            page_url: candidate.page_url.clone(),
            variants: vec![variant],
            preferred_variant_id: Some(preferred),
            audio_tracks: vec![],
            subtitle_tracks: vec![],
            is_vod: true,
            segment_count: None,
            duration_seconds: None,
            content_length: None,
            observed_at: candidate.observed_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::hls::HLSParser;
    use std::fs;
    use std::path::PathBuf;
    use url::Url;

    fn fixture(rel: &str) -> String {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("../../fixtures");
        path.push(rel);
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn classifies_hls_mp4_segment() {
        let hls = Url::parse("https://u14.vidzy.cc/list/abc.m3u8").unwrap();
        let mp4 = Url::parse("https://cdn.example.com/film.mp4").unwrap();
        let ts = Url::parse("https://u14.vidzy.cc/seg/1.ts").unwrap();
        assert_eq!(MediaDetector::classify(&hls, None), MediaClassification::Hls);
        assert_eq!(
            MediaDetector::classify(&mp4, None),
            MediaClassification::Progressive
        );
        assert_eq!(
            MediaDetector::classify(&ts, None),
            MediaClassification::Segment
        );
    }

    #[test]
    fn preferred_variant_highest_bandwidth() {
        let url = Url::parse("https://cdn.example.com/video/master.m3u8").unwrap();
        let candidate = NetworkMediaCandidate::new(url.clone(), "manual");
        let text = fixture("hls/master.m3u8");
        let playlist = HLSParser::parse(&text, &url).unwrap();
        let media = MediaDetector::make_hls(&candidate, &playlist, Some("Demo".into()));
        assert_eq!(media.kind, ManifestKind::Hls);
        assert_eq!(media.variants.len(), 3);
        assert_eq!(media.preferred_variant().unwrap().bandwidth, Some(2_800_000));
        assert!(media.preferred_variant().unwrap().display_label().contains("1920"));
        assert_eq!(media.display_title(), "Demo");
    }
}
