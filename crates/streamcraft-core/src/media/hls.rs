use std::collections::HashMap;

use thiserror::Error;
use url::Url;

use super::models::Resolution;

#[derive(Debug, Error)]
pub enum HLSParserError {
    #[error("empty HLS playlist")]
    EmptyDocument,
    #[error("missing #EXTM3U header")]
    MissingHeader,
    #[error("invalid HLS URI: {0}")]
    InvalidUri(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HLSPlaylistKind {
    Master,
    Media,
}

#[derive(Debug, Clone)]
pub struct HLSStreamInf {
    pub bandwidth: Option<i64>,
    pub average_bandwidth: Option<i64>,
    pub resolution: Option<Resolution>,
    pub codecs: Option<String>,
    pub uri: Url,
}

#[derive(Debug, Clone)]
pub struct HLSSegment {
    pub duration: f64,
    pub uri: Url,
    pub byte_range: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HLSMediaPlaylist {
    pub target_duration: Option<f64>,
    pub media_sequence: Option<i64>,
    pub has_end_list: bool,
    pub map_uri: Option<Url>,
    pub segments: Vec<HLSSegment>,
}

impl HLSMediaPlaylist {
    pub fn total_duration(&self) -> f64 {
        self.segments.iter().map(|s| s.duration).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HLSRenditionKind {
    Audio,
    Subtitles,
    ClosedCaptions,
    Video,
}

#[derive(Debug, Clone)]
pub struct HLSMediaRendition {
    pub kind: HLSRenditionKind,
    pub group_id: String,
    pub name: Option<String>,
    pub language: Option<String>,
    pub uri: Option<Url>,
    pub is_default: bool,
    pub is_auto_select: bool,
}

#[derive(Debug, Clone)]
pub struct HLSPlaylist {
    pub kind: HLSPlaylistKind,
    pub base_url: Url,
    pub streams: Vec<HLSStreamInf>,
    pub media: Option<HLSMediaPlaylist>,
    pub renditions: Vec<HLSMediaRendition>,
}

pub struct HLSParser;

impl HLSParser {
    pub fn parse(text: &str, playlist_url: &Url) -> Result<HLSPlaylist, HLSParserError> {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = normalized
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("#EXT-X-COMMENT"))
            .collect();
        let first = lines.first().ok_or(HLSParserError::EmptyDocument)?;
        if *first != "#EXTM3U" && !first.starts_with("#EXTM3U") {
            return Err(HLSParserError::MissingHeader);
        }
        let is_master = lines.iter().any(|l| l.starts_with("#EXT-X-STREAM-INF"));
        if is_master {
            Self::parse_master(&lines, playlist_url)
        } else {
            Self::parse_media(&lines, playlist_url)
        }
    }

    fn parse_master(lines: &[&str], playlist_url: &Url) -> Result<HLSPlaylist, HLSParserError> {
        let mut streams = Vec::new();
        let mut renditions = Vec::new();
        let mut index = 0usize;
        while index < lines.len() {
            let line = lines[index];
            if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA:") {
                let attrs = parse_attributes(rest);
                let kind = match attrs.get("TYPE").map(|s| s.to_uppercase()).as_deref() {
                    Some("AUDIO") => Some(HLSRenditionKind::Audio),
                    Some("SUBTITLES") => Some(HLSRenditionKind::Subtitles),
                    Some("CLOSED-CAPTIONS") => Some(HLSRenditionKind::ClosedCaptions),
                    Some("VIDEO") => Some(HLSRenditionKind::Video),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let uri = attrs
                        .get("URI")
                        .and_then(|u| resolve_uri(u, playlist_url).ok());
                    renditions.push(HLSMediaRendition {
                        kind,
                        group_id: attrs.get("GROUP-ID").cloned().unwrap_or_default(),
                        name: attrs.get("NAME").cloned(),
                        language: attrs.get("LANGUAGE").cloned(),
                        uri,
                        is_default: attrs
                            .get("DEFAULT")
                            .map(|v| v.eq_ignore_ascii_case("YES"))
                            .unwrap_or(false),
                        is_auto_select: attrs
                            .get("AUTOSELECT")
                            .map(|v| v.eq_ignore_ascii_case("YES"))
                            .unwrap_or(false),
                    });
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                let attrs = parse_attributes(rest);
                index += 1;
                if index >= lines.len() {
                    break;
                }
                let uri_line = lines[index];
                if uri_line.starts_with('#') {
                    continue;
                }
                let uri = resolve_uri(uri_line, playlist_url)?;
                streams.push(HLSStreamInf {
                    bandwidth: attrs.get("BANDWIDTH").and_then(|v| v.parse().ok()),
                    average_bandwidth: attrs
                        .get("AVERAGE-BANDWIDTH")
                        .and_then(|v| v.parse().ok()),
                    resolution: parse_resolution(attrs.get("RESOLUTION").map(String::as_str)),
                    codecs: attrs.get("CODECS").cloned(),
                    uri,
                });
            }
            index += 1;
        }
        Ok(HLSPlaylist {
            kind: HLSPlaylistKind::Master,
            base_url: playlist_url.clone(),
            streams,
            media: None,
            renditions,
        })
    }

    fn parse_media(lines: &[&str], playlist_url: &Url) -> Result<HLSPlaylist, HLSParserError> {
        let mut target_duration = None;
        let mut media_sequence = None;
        let mut has_end_list = false;
        let mut map_uri = None;
        let mut segments = Vec::new();
        let mut pending_duration = None;
        let mut pending_byte_range = None;

        for line in lines {
            if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
                target_duration = rest.parse().ok();
            } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                media_sequence = rest.parse().ok();
            } else if *line == "#EXT-X-ENDLIST" {
                has_end_list = true;
            } else if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
                let attrs = parse_attributes(rest);
                if let Some(uri) = attrs.get("URI") {
                    map_uri = Some(resolve_uri(&unquote(uri), playlist_url)?);
                }
            } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
                let duration_part = rest.split(',').next().unwrap_or(rest);
                pending_duration = duration_part.parse().ok();
            } else if let Some(rest) = line.strip_prefix("#EXT-X-BYTERANGE:") {
                pending_byte_range = Some(rest.to_string());
            } else if line.starts_with('#') {
                continue;
            } else if let Some(duration) = pending_duration.take() {
                let uri = resolve_uri(line, playlist_url)?;
                segments.push(HLSSegment {
                    duration,
                    uri,
                    byte_range: pending_byte_range.take(),
                });
            }
        }

        Ok(HLSPlaylist {
            kind: HLSPlaylistKind::Media,
            base_url: playlist_url.clone(),
            streams: vec![],
            media: Some(HLSMediaPlaylist {
                target_duration,
                media_sequence,
                has_end_list,
                map_uri,
                segments,
            }),
            renditions: vec![],
        })
    }
}

pub fn resolve_uri(raw: &str, base: &Url) -> Result<Url, HLSParserError> {
    let trimmed = raw.trim();
    if let Ok(absolute) = Url::parse(trimmed) {
        if absolute.scheme() == "http"
            || absolute.scheme() == "https"
            || absolute.scheme() == "file"
        {
            return Ok(absolute);
        }
    }
    base.join(trimmed)
        .map_err(|_| HLSParserError::InvalidUri(trimmed.to_string()))
}

fn parse_attributes(raw: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut current_key = String::new();
    let mut current_value = String::new();
    let mut in_key = true;
    let mut is_quoted = false;

    let commit = |key: &mut String, value: &mut String, out: &mut HashMap<String, String>| {
        let k = key.trim().to_uppercase();
        if !k.is_empty() {
            out.insert(k, unquote(value.trim()));
        }
        key.clear();
        value.clear();
    };

    for ch in raw.chars() {
        if in_key {
            if ch == '=' {
                in_key = false;
                is_quoted = false;
            } else {
                current_key.push(ch);
            }
        } else if current_value.is_empty() && ch == '"' {
            is_quoted = true;
            current_value.push(ch);
        } else if is_quoted {
            current_value.push(ch);
            if ch == '"' {
                is_quoted = false;
            }
        } else if ch == ',' {
            commit(&mut current_key, &mut current_value, &mut result);
            in_key = true;
        } else {
            current_value.push(ch);
        }
    }
    commit(&mut current_key, &mut current_value, &mut result);
    result
}

fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn parse_resolution(raw: Option<&str>) -> Option<Resolution> {
    let raw = raw?;
    let lower = raw.to_lowercase();
    let mut parts = lower.split('x');
    let w = parts.next()?.parse().ok()?;
    let h = parts.next()?.parse().ok()?;
    Some(Resolution {
        width: w,
        height: h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(rel: &str) -> String {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("../../fixtures");
        path.push(rel);
        fs::read_to_string(path).expect("fixture")
    }

    #[test]
    fn parses_master_playlist() {
        let text = fixture("hls/master.m3u8");
        let url = Url::parse("https://cdn.example.com/video/master.m3u8").unwrap();
        let playlist = HLSParser::parse(&text, &url).unwrap();
        assert_eq!(playlist.kind, HLSPlaylistKind::Master);
        assert_eq!(playlist.streams.len(), 3);
        assert_eq!(
            playlist
                .streams
                .iter()
                .map(|s| s.bandwidth)
                .collect::<Vec<_>>(),
            vec![Some(800_000), Some(1_400_000), Some(2_800_000)]
        );
        let r = playlist.streams[1].resolution.unwrap();
        assert_eq!((r.width, r.height), (1280, 720));
        assert!(playlist.streams[2].uri.as_str().ends_with("1080p.m3u8"));
    }

    #[test]
    fn parses_media_vod() {
        let text = fixture("hls/media_vod.m3u8");
        let url = Url::parse("https://cdn.example.com/video/index.m3u8").unwrap();
        let playlist = HLSParser::parse(&text, &url).unwrap();
        assert_eq!(playlist.kind, HLSPlaylistKind::Media);
        let media = playlist.media.unwrap();
        assert!(media.has_end_list);
        assert_eq!(media.segments.len(), 3);
        assert!(media.segments[0].uri.as_str().ends_with("seg0.ts"));
        assert!((media.total_duration() - 26.5).abs() < 0.001);
    }

    #[test]
    fn parses_fmp4_with_map() {
        let text = fixture("hls/media_fmp4.m3u8");
        let url = Url::parse("https://cdn.example.com/fmp4/index.m3u8").unwrap();
        let playlist = HLSParser::parse(&text, &url).unwrap();
        let media = playlist.media.unwrap();
        assert!(media.map_uri.unwrap().as_str().ends_with("init.mp4"));
        assert_eq!(media.segments.len(), 2);
        assert!(media.has_end_list);
    }

    #[test]
    fn resolves_relative_master_uris() {
        let text = fixture("hls/master_relative.m3u8");
        let url = Url::parse("https://cdn.example.com/video/live/master.m3u8").unwrap();
        let playlist = HLSParser::parse(&text, &url).unwrap();
        assert_eq!(playlist.streams.len(), 2);
        assert_eq!(
            playlist.streams[0].uri.as_str(),
            "https://cdn.example.com/video/live/relative/720.m3u8"
        );
        assert_eq!(
            playlist.streams[1].uri.as_str(),
            "https://cdn.example.com/video/1080/index.m3u8"
        );
    }
}
