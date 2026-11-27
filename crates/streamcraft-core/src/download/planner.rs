use std::path::PathBuf;

use thiserror::Error;
use url::Url;

use crate::media::cdn_gate::{apply_gate_token, gate_token};
use crate::media::dash::{DASHParser, DASHSegmentRef};
use crate::media::hls::{HLSParser, HLSPlaylistKind};
use crate::media::models::{ManifestKind, MediaVariant};

#[derive(Debug, Error)]
pub enum PlanError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Hls(#[from] crate::media::hls::HLSParserError),
    #[error(transparent)]
    Dash(#[from] crate::media::dash::DASHParserError),
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
    /// Inclusive `(first, last)` byte offsets (HLS EXT-X-BYTERANGE).
    pub byte_range: Option<(u64, u64)>,
}

#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub units: Vec<DownloadUnit>,
    pub kind: ManifestKind,
    pub local_playlist_hint: Option<PathBuf>,
}

/// `source` is the detected media URL: the MPD for DASH, and the scope of `cookie`
/// (only sent to the same site, see [`cookie_allowed`]).
pub async fn plan_for_variant(
    client: &reqwest::Client,
    variant: &MediaVariant,
    kind: ManifestKind,
    source: &Url,
    referer: &str,
    cookie: Option<&str>,
) -> Result<DownloadPlan, PlanError> {
    match kind {
        ManifestKind::Progressive => Ok(DownloadPlan {
            units: vec![DownloadUnit {
                index: 0,
                url: variant.playlist_url.clone(),
                file_name: "media.mp4".into(),
                duration: None,
                is_init: false,
                byte_range: None,
            }],
            kind,
            local_playlist_hint: None,
        }),
        ManifestKind::Hls => {
            plan_hls(client, &variant.playlist_url, source, referer, cookie, 0).await
        }
        ManifestKind::Dash => {
            let xml = fetch_text(client, source, source, referer, cookie).await?;
            plan_dash_xml(&xml, source, variant)
        }
    }
}

/// Cookie is sent only to the source host or a host sharing its last two labels.
/// ponytail: no public-suffix list (a.co.uk ~ b.co.uk); use the `psl` crate if that matters.
pub fn cookie_allowed(target: &Url, source: &Url) -> bool {
    use url::Host;
    match (target.host(), source.host()) {
        (Some(Host::Domain(t)), Some(Host::Domain(s))) => {
            t.eq_ignore_ascii_case(s) || last_two_labels(t).is_some_and(|x| Some(x) == last_two_labels(s))
        }
        (Some(t), Some(s)) => t == s,
        _ => false,
    }
}

fn last_two_labels(host: &str) -> Option<&str> {
    let mut dots = host.trim_end_matches('.').rmatch_indices('.');
    dots.next()?;
    Some(dots.next().map_or(host, |(i, _)| &host[i + 1..]))
}

async fn fetch_text(
    client: &reqwest::Client,
    url: &Url,
    source: &Url,
    referer: &str,
    cookie: Option<&str>,
) -> Result<String, PlanError> {
    let mut req = client.get(url.clone()).header("Referer", referer);
    if let Some(c) = cookie.filter(|c| !c.is_empty() && cookie_allowed(url, source)) {
        req = req.header("Cookie", c);
    }
    Ok(req.send().await?.error_for_status()?.text().await?)
}

/// Video representation matching `variant` (else highest bandwidth) + best audio,
/// audio units prefixed `audio/` (muxed by `remux::assemble`).
fn plan_dash_xml(xml: &str, mpd_url: &Url, variant: &MediaVariant) -> Result<DownloadPlan, PlanError> {
    if xml.contains("type=\"dynamic\"") {
        return Err(PlanError::Message(
            "live DASH (dynamic MPD) is not supported".into(),
        ));
    }
    let m = DASHParser::parse(xml, mpd_url)?;
    if m.has_content_protection {
        return Err(PlanError::Message(
            "DASH stream is DRM-protected (ContentProtection): download refused".into(),
        ));
    }
    let reps = &m.video_representations;
    let video = reps
        .iter()
        .find(|r| variant.dash_representation_id.as_deref() == Some(r.id.as_str()))
        .or_else(|| reps.iter().max_by_key(|r| r.bandwidth.unwrap_or(0)))
        .ok_or_else(|| PlanError::Message("no DASH video representation".into()))?;
    let audio = m
        .audio_representations
        .iter()
        .max_by_key(|r| r.bandwidth.unwrap_or(0));
    let mut units = Vec::new();
    push_dash_units(&mut units, &video.segments, "");
    if let Some(a) = audio {
        push_dash_units(&mut units, &a.segments, "audio/");
    }
    Ok(DownloadPlan {
        units,
        kind: ManifestKind::Dash,
        local_playlist_hint: None,
    })
}

fn push_dash_units(units: &mut Vec<DownloadUnit>, segments: &[DASHSegmentRef], prefix: &str) {
    for s in segments {
        let index = units.len();
        let name = if s.is_initialization { "init.mp4" } else { "seg.m4s" };
        units.push(DownloadUnit {
            index,
            url: s.url.clone(),
            file_name: format!("{prefix}{index:05}_{name}"),
            duration: None,
            is_init: s.is_initialization,
            byte_range: None,
        });
    }
}

/// EXT-X-BYTERANGE `len[@offset]` → inclusive range; without `@offset` the range
/// starts at `next` (byte after the previous sub-range, RFC 8216 §4.3.2.2).
fn parse_byte_range(raw: &str, next: u64) -> Option<(u64, u64)> {
    let (len, offset) = match raw.trim().split_once('@') {
        Some((l, o)) => (l, Some(o.trim().parse().ok()?)),
        None => (raw.trim(), None),
    };
    let len: u64 = len.trim().parse().ok()?;
    let start = offset.unwrap_or(next);
    (len > 0).then(|| (start, start + len - 1))
}

async fn plan_hls(
    client: &reqwest::Client,
    url: &Url,
    source: &Url,
    referer: &str,
    cookie: Option<&str>,
    depth: u8,
) -> Result<DownloadPlan, PlanError> {
    let gate = gate_token(url);
    let text = fetch_text(client, url, source, referer, cookie).await?;
    let playlist = HLSParser::parse(&text, url)?;
    match playlist.kind {
        HLSPlaylistKind::Master => {
            if depth >= 3 {
                return Err(PlanError::Message("HLS master playlists nested too deep".into()));
            }
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
            let child = if uri_s.contains("index-a") && !uri_s.contains("index-v") {
                let video = playlist
                    .streams
                    .iter()
                    .filter(|s| {
                        let u = s.uri.as_str().to_lowercase();
                        !u.contains("index-a") || u.contains("index-v")
                    })
                    .max_by_key(|s| s.bandwidth.unwrap_or(0));
                video.map(|v| &v.uri).unwrap_or(&best.uri)
            } else {
                &best.uri
            };
            let child = match gate.as_deref() {
                Some(tok) => apply_gate_token(child, tok),
                None => child.clone(),
            };
            Box::pin(plan_hls(client, &child, source, referer, cookie, depth + 1)).await
        }
        HLSPlaylistKind::Media => {
            let media = playlist
                .media
                .ok_or_else(|| PlanError::Message("media playlist missing body".into()))?;
            let mut units = Vec::new();
            let mut index = 0usize;
            let mut next_byte = 0u64;
            let ranges: Vec<_> = media
                .segments
                .iter()
                .map(|seg| {
                    let r = seg.byte_range.as_deref().and_then(|b| parse_byte_range(b, next_byte));
                    if let Some((_, end)) = r {
                        next_byte = end + 1;
                    }
                    r
                })
                .collect();
            let fmp4 = media.map_uri.is_some();
            if let Some(map) = media.map_uri {
                // ponytail: EXT-X-MAP BYTERANGE isn't parsed by hls.rs; assume the init
                // section is the head of the file before the first segment's range.
                let byte_range = match (media.segments.first(), ranges.first()) {
                    (Some(first), Some(Some((start, _)))) if first.uri == map && *start > 0 => {
                        Some((0, start - 1))
                    }
                    _ => None,
                };
                let map_url = match gate.as_deref() {
                    Some(tok) => apply_gate_token(&map, tok),
                    None => map,
                };
                units.push(DownloadUnit {
                    index,
                    url: map_url,
                    file_name: format!("{index:05}_init.mp4"),
                    duration: None,
                    is_init: true,
                    byte_range,
                });
                index += 1;
            }
            for (seg, byte_range) in media.segments.into_iter().zip(ranges) {
                let ext = seg
                    .uri
                    .path_segments()
                    .and_then(|mut s| s.next_back())
                    .and_then(|n| n.rsplit_once('.'))
                    .map(|(_, e)| e.to_lowercase())
                    .unwrap_or_else(|| "ts".into());
                let file_ext = local_segment_ext(&ext, fmp4);
                let seg_url = match gate.as_deref() {
                    Some(tok) => apply_gate_token(&seg.uri, tok),
                    None => seg.uri.clone(),
                };
                units.push(DownloadUnit {
                    index,
                    url: seg_url,
                    file_name: format!("{index:05}_seg.{file_ext}"),
                    duration: Some(seg.duration),
                    is_init: false,
                    byte_range,
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


/// Extension for the local copy of a segment. ffmpeg ≥ 7 refuses HLS segments
/// whose extension isn't a media one, and CDNs disguise TS/fMP4 as `.jpg`,
/// `.image`, `.js`…: keep real media extensions, otherwise name by container
/// (the HLS demuxer probes the content anyway).
fn local_segment_ext(ext: &str, fmp4: bool) -> String {
    const MEDIA: &[&str] = &["ts", "m4s", "mp4", "m4a", "m4v", "aac", "ac3", "ec3", "mp3", "cmfv", "cmfa"];
    if MEDIA.contains(&ext) {
        ext.to_string()
    } else if fmp4 {
        "m4s".into()
    } else {
        "ts".into()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_range_offset_defaults_to_previous_end() {
        assert_eq!(parse_byte_range("100@50", 0), Some((50, 149)));
        assert_eq!(parse_byte_range("10", 150), Some((150, 159)));
        assert_eq!(parse_byte_range("0", 0), None);
        assert_eq!(parse_byte_range("x@1", 0), None);
    }

    #[test]
    fn cookie_scope() {
        let src = Url::parse("https://www.example.com/watch").unwrap();
        let ok = |u: &str| cookie_allowed(&Url::parse(u).unwrap(), &src);
        assert!(ok("https://www.example.com/a.ts"));
        assert!(ok("https://cdn.example.com/a.ts"));
        assert!(ok("https://example.com/a.ts"));
        assert!(!ok("https://cdn.other.net/a.ts"));
        assert!(!ok("https://example.com.evil.net/a.ts"));
        let ip = Url::parse("http://10.0.0.1/x").unwrap();
        assert!(!cookie_allowed(&Url::parse("http://192.168.0.1/x").unwrap(), &ip));
    }

    #[test]
    fn dash_template_mpd_to_units() {
        let xml = r#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
<BaseURL>https://cdn.example.com/d/</BaseURL>
<AdaptationSet contentType="video" mimeType="video/mp4">
 <SegmentTemplate timescale="1000" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Time$.m4s">
  <SegmentTimeline><S t="0" d="4000" r="1"/></SegmentTimeline>
 </SegmentTemplate>
 <Representation id="v1" bandwidth="500000" width="640" height="360"/>
 <Representation id="v2" bandwidth="900000" width="1280" height="720"/>
</AdaptationSet>
<AdaptationSet contentType="audio" mimeType="audio/mp4">
 <SegmentTemplate timescale="1" duration="4" startNumber="1" initialization="a/init.mp4" media="a/$Number%03d$.m4s"/>
 <Representation id="a1" bandwidth="128000"/>
</AdaptationSet></Period></MPD>"#;
        let mpd = Url::parse("https://cdn.example.com/d/manifest.mpd").unwrap();
        let mut variant = MediaVariant::new(mpd.clone());
        let plan = plan_dash_xml(xml, &mpd, &variant).unwrap();
        let got: Vec<_> = plan
            .units
            .iter()
            .map(|u| (u.index, u.is_init, u.file_name.as_str(), u.url.path()))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, true, "00000_init.mp4", "/d/v2/init.mp4"),
                (1, false, "00001_seg.m4s", "/d/v2/0.m4s"),
                (2, false, "00002_seg.m4s", "/d/v2/4000.m4s"),
                (3, true, "audio/00003_init.mp4", "/d/a/init.mp4"),
                (4, false, "audio/00004_seg.m4s", "/d/a/001.m4s"),
                (5, false, "audio/00005_seg.m4s", "/d/a/002.m4s"),
            ]
        );
        variant.dash_representation_id = Some("v1".into());
        let plan = plan_dash_xml(xml, &mpd, &variant).unwrap();
        assert_eq!(plan.units[0].url.path(), "/d/v1/init.mp4");

        let live = xml.replace("static", "dynamic");
        assert!(plan_dash_xml(&live, &mpd, &variant).is_err());
    }

    #[test]
    fn lure_extensions_get_a_media_extension() {
        assert_eq!(local_segment_ext("jpg", false), "ts");
        assert_eq!(local_segment_ext("image", false), "ts");
        assert_eq!(local_segment_ext("js", true), "m4s");
        assert_eq!(local_segment_ext("m4s", true), "m4s");
        assert_eq!(local_segment_ext("ts", false), "ts");
    }
}
