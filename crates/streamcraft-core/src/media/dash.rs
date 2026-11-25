//! Minimal DASH MPD parser (SegmentList + SegmentTemplate + BaseURL).

use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum DASHParserError {
    #[error("empty DASH document")]
    EmptyDocument,
    #[error("invalid DASH XML: {0}")]
    InvalidXml(String),
    #[error("no video representation in MPD")]
    NoVideoRepresentation,
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone)]
pub struct DASHSegmentRef {
    pub url: Url,
    pub is_initialization: bool,
}

#[derive(Debug, Clone)]
pub struct DASHRepresentation {
    pub id: String,
    pub bandwidth: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub codecs: Option<String>,
    pub mime_type: Option<String>,
    pub segments: Vec<DASHSegmentRef>,
    pub language: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DASHManifest {
    pub base_url: Url,
    pub video_representations: Vec<DASHRepresentation>,
    pub audio_representations: Vec<DASHRepresentation>,
    pub text_representations: Vec<DASHRepresentation>,
    pub period_duration_seconds: Option<f64>,
    pub has_content_protection: bool,
}

pub struct DASHParser;

impl DASHParser {
    pub fn parse(xml: &str, manifest_url: &Url) -> Result<DASHManifest, DASHParserError> {
        let trimmed = xml.trim();
        if trimmed.is_empty() {
            return Err(DASHParserError::EmptyDocument);
        }

        let has_content_protection = trimmed.contains("ContentProtection")
            || trimmed.contains("cenc:default_KID")
            || trimmed.to_lowercase().contains("urn:mpeg:cenc");

        let period_duration = extract_attr(trimmed, "mediaPresentationDuration")
            .or_else(|| {
                // Period duration attribute (first occurrence)
                find_tag_attr(trimmed, "Period", "duration")
            })
            .and_then(|d| parse_duration(&d));

        let mut base = manifest_url.clone();
        if let Some(bu) = extract_inner_text(trimmed, "BaseURL") {
            if let Ok(resolved) = resolve_uri(&bu, &base) {
                base = resolved;
            }
        }

        let mut video = Vec::new();
        let mut audio = Vec::new();
        let mut text = Vec::new();

        // Walk AdaptationSet blocks
        for adaptation in split_tags(trimmed, "AdaptationSet") {
            let content_type = find_tag_attr(&adaptation, "AdaptationSet", "contentType")
                .or_else(|| find_tag_attr(&adaptation, "AdaptationSet", "mimeType"))
                .unwrap_or_default()
                .to_lowercase();
            let lang = find_tag_attr(&adaptation, "AdaptationSet", "lang");
            let label = find_tag_attr(&adaptation, "AdaptationSet", "label")
                .or_else(|| extract_inner_text(&adaptation, "Label"));

            let adaptation_base = if let Some(bu) = extract_inner_text(&adaptation, "BaseURL") {
                resolve_uri(&bu, &base).unwrap_or(base.clone())
            } else {
                base.clone()
            };

            for representation in split_tags(&adaptation, "Representation") {
                let id = find_tag_attr(&representation, "Representation", "id")
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let bandwidth = find_tag_attr(&representation, "Representation", "bandwidth")
                    .and_then(|v| v.parse().ok());
                let width = find_tag_attr(&representation, "Representation", "width")
                    .or_else(|| find_tag_attr(&adaptation, "AdaptationSet", "width"))
                    .and_then(|v| v.parse().ok());
                let height = find_tag_attr(&representation, "Representation", "height")
                    .or_else(|| find_tag_attr(&adaptation, "AdaptationSet", "height"))
                    .and_then(|v| v.parse().ok());
                let codecs = find_tag_attr(&representation, "Representation", "codecs")
                    .or_else(|| find_tag_attr(&adaptation, "AdaptationSet", "codecs"));
                let mime_type = find_tag_attr(&representation, "Representation", "mimeType")
                    .or_else(|| find_tag_attr(&adaptation, "AdaptationSet", "mimeType"));

                let rep_base = if let Some(bu) = extract_inner_text(&representation, "BaseURL") {
                    resolve_uri(&bu, &adaptation_base).unwrap_or(adaptation_base.clone())
                } else {
                    adaptation_base.clone()
                };

                let segments = resolve_segments(
                    &representation,
                    &adaptation,
                    &rep_base,
                    period_duration,
                    &id,
                    bandwidth.unwrap_or(0).to_string().as_str(),
                )?;

                let rep = DASHRepresentation {
                    id,
                    bandwidth,
                    width,
                    height,
                    codecs: codecs.clone(),
                    mime_type: mime_type.clone(),
                    segments,
                    language: lang.clone(),
                    label: label.clone(),
                };

                let mime = mime_type.unwrap_or_default().to_lowercase();
                let codecs_l = codecs.unwrap_or_default().to_lowercase();
                if mime.contains("video") || (width.is_some() && height.is_some()) {
                    video.push(rep);
                } else if mime.contains("audio") || content_type.contains("audio") {
                    audio.push(rep);
                } else if mime.contains("text")
                    || mime.contains("ttml")
                    || mime.contains("vtt")
                    || content_type.contains("text")
                {
                    text.push(rep);
                } else if content_type.contains("video") {
                    video.push(rep);
                } else if codecs_l.contains("avc") || codecs_l.contains("hvc") {
                    video.push(rep);
                } else if codecs_l.contains("mp4a") {
                    audio.push(rep);
                } else {
                    video.push(rep);
                }
            }
        }

        if video.is_empty() {
            return Err(DASHParserError::NoVideoRepresentation);
        }

        Ok(DASHManifest {
            base_url: manifest_url.clone(),
            video_representations: video,
            audio_representations: audio,
            text_representations: text,
            period_duration_seconds: period_duration,
            has_content_protection,
        })
    }
}

fn resolve_segments(
    representation: &str,
    adaptation: &str,
    base_url: &Url,
    period_duration: Option<f64>,
    representation_id: &str,
    bandwidth: &str,
) -> Result<Vec<DASHSegmentRef>, DASHParserError> {
    if let Some(list) = first_block(representation, "SegmentList")
        .or_else(|| first_block(adaptation, "SegmentList"))
    {
        return parse_segment_list(&list, base_url);
    }
    if let Some(template) = first_block(representation, "SegmentTemplate")
        .or_else(|| first_block(adaptation, "SegmentTemplate"))
    {
        return parse_segment_template(
            &template,
            base_url,
            period_duration,
            representation_id,
            bandwidth,
        );
    }
    if let Some(bu) = extract_inner_text(representation, "BaseURL")
        .or_else(|| extract_inner_text(adaptation, "BaseURL"))
    {
        let url = resolve_uri(&bu, base_url)
            .map_err(|e| DASHParserError::Message(e))?;
        return Ok(vec![DASHSegmentRef {
            url,
            is_initialization: false,
        }]);
    }
    Err(DASHParserError::Message(
        "Representation without SegmentList/Template/BaseURL".into(),
    ))
}

fn parse_segment_list(list: &str, base_url: &Url) -> Result<Vec<DASHSegmentRef>, DASHParserError> {
    let mut segments = Vec::new();
    if let Some(init) = first_block(list, "Initialization") {
        if let Some(source) = find_tag_attr(&init, "Initialization", "sourceURL")
            .or_else(|| find_tag_attr(&init, "Initialization", "sourceUrl"))
        {
            segments.push(DASHSegmentRef {
                url: resolve_uri(&source, base_url)
                    .map_err(DASHParserError::Message)?,
                is_initialization: true,
            });
        }
    }
    for url_el in split_tags(list, "SegmentURL") {
        if let Some(media) = find_tag_attr(&url_el, "SegmentURL", "media") {
            segments.push(DASHSegmentRef {
                url: resolve_uri(&media, base_url).map_err(DASHParserError::Message)?,
                is_initialization: false,
            });
        }
    }
    if segments.is_empty() {
        return Err(DASHParserError::Message("empty SegmentList".into()));
    }
    Ok(segments)
}

fn parse_segment_template(
    template: &str,
    base_url: &Url,
    period_duration: Option<f64>,
    representation_id: &str,
    bandwidth: &str,
) -> Result<Vec<DASHSegmentRef>, DASHParserError> {
    let mut segments = Vec::new();
    let timescale = extract_attr_from_open(template, "timescale")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0);
    let start_number = extract_attr_from_open(template, "startNumber")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1);
    let duration_attr = extract_attr_from_open(template, "duration").and_then(|v| v.parse::<f64>().ok());

    if let Some(init_template) = extract_attr_from_open(template, "initialization") {
        let path = expand_template(&init_template, start_number, representation_id, bandwidth, 0);
        segments.push(DASHSegmentRef {
            url: resolve_uri(&path, base_url).map_err(DASHParserError::Message)?,
            is_initialization: true,
        });
    }

    let Some(media_template) = extract_attr_from_open(template, "media") else {
        if segments.is_empty() {
            return Err(DASHParserError::Message("incomplete SegmentTemplate".into()));
        }
        return Ok(segments);
    };

    if let (Some(duration_attr), Some(period_duration)) = (duration_attr, period_duration) {
        if duration_attr > 0.0 && period_duration > 0.0 && timescale > 0.0 {
            let segment_duration_seconds = duration_attr / timescale;
            let count = ((period_duration / segment_duration_seconds).ceil() as i64).max(1);
            for i in 0..count {
                let number = start_number + i;
                let path = expand_template(
                    &media_template,
                    number,
                    representation_id,
                    bandwidth,
                    (i as f64 * duration_attr) as i64,
                );
                segments.push(DASHSegmentRef {
                    url: resolve_uri(&path, base_url).map_err(DASHParserError::Message)?,
                    is_initialization: false,
                });
            }
            return Ok(segments);
        }
    }

    let path = expand_template(&media_template, start_number, representation_id, bandwidth, 0);
    segments.push(DASHSegmentRef {
        url: resolve_uri(&path, base_url).map_err(DASHParserError::Message)?,
        is_initialization: false,
    });
    Ok(segments)
}

fn expand_template(
    template: &str,
    number: i64,
    representation_id: &str,
    bandwidth: &str,
    time: i64,
) -> String {
    let placeholder = "\u{FFF0}";
    let mut result = template.replace("$$", placeholder);
    result = result.replace("$RepresentationID$", representation_id);
    result = result.replace("$Bandwidth$", bandwidth);
    result = result.replace("$Number%05d$", &format!("{number:05}"));
    result = result.replace("$Number$", &number.to_string());
    result = result.replace("$Time$", &time.to_string());
    result.replace(placeholder, "$")
}

fn resolve_uri(raw: &str, base: &Url) -> Result<Url, String> {
    let trimmed = raw.trim();
    if let Ok(absolute) = Url::parse(trimmed) {
        if matches!(absolute.scheme(), "http" | "https" | "file") {
            return Ok(absolute);
        }
    }
    base.join(trimmed).map_err(|e| e.to_string())
}

fn parse_duration(raw: &str) -> Option<f64> {
    // PT20S / PT1M30.5S
    let s = raw.trim();
    if !s.starts_with("PT") {
        return s.parse().ok();
    }
    let mut rest = &s[2..];
    let mut total = 0.0;
    if let Some(idx) = rest.find('H') {
        total += rest[..idx].parse::<f64>().ok()? * 3600.0;
        rest = &rest[idx + 1..];
    }
    if let Some(idx) = rest.find('M') {
        // Distinguish minutes from milliseconds ending — only if followed by S or end
        total += rest[..idx].parse::<f64>().ok()? * 60.0;
        rest = &rest[idx + 1..];
    }
    if let Some(idx) = rest.find('S') {
        total += rest[..idx].parse::<f64>().ok()?;
    }
    Some(total)
}

fn extract_attr(doc: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = doc.find(&needle)? + needle.len();
    let end = doc[start..].find('"')? + start;
    Some(doc[start..end].to_string())
}

fn extract_attr_from_open(block: &str, name: &str) -> Option<String> {
    extract_attr(block, name)
}

fn find_tag_attr(doc: &str, tag: &str, attr: &str) -> Option<String> {
    let open = format!("<{tag}");
    let idx = doc.find(&open)?;
    let rest = &doc[idx..];
    let end = rest.find('>')?;
    extract_attr(&rest[..=end], attr)
}

fn extract_inner_text(doc: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start_idx = doc.find(&open)?;
    let after_open = doc[start_idx..].find('>')? + start_idx + 1;
    let close = format!("</{tag}>");
    let end_idx = doc[after_open..].find(&close)? + after_open;
    Some(doc[after_open..end_idx].trim().to_string())
}

fn first_block(doc: &str, tag: &str) -> Option<String> {
    split_tags(doc, tag).into_iter().next()
}

fn split_tags(doc: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = doc[search_from..].find(&open) {
        let start = search_from + rel;
        // self-closing?
        let after = &doc[start..];
        if let Some(gt) = after.find('>') {
            if after.as_bytes().get(gt.saturating_sub(1)) == Some(&b'/') {
                out.push(after[..=gt].to_string());
                search_from = start + gt + 1;
                continue;
            }
        }
        if let Some(rel_end) = doc[start..].find(&close) {
            let end = start + rel_end + close.len();
            out.push(doc[start..end].to_string());
            search_from = end;
        } else {
            break;
        }
    }
    out
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
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn parses_template_mpd() {
        let xml = fixture("dash/template.mpd");
        let url = Url::parse("https://cdn.example.com/dash/manifest.mpd").unwrap();
        let m = DASHParser::parse(&xml, &url).unwrap();
        assert_eq!(m.video_representations.len(), 1);
        assert_eq!(m.period_duration_seconds, Some(20.0));
        let segs = &m.video_representations[0].segments;
        assert!(segs.iter().any(|s| s.is_initialization));
        assert!(segs.len() >= 2);
        assert!(segs[0].url.as_str().contains("init-v1.mp4"));
    }

    #[test]
    fn parse_duration_pt20s() {
        assert_eq!(parse_duration("PT20S"), Some(20.0));
        assert_eq!(parse_duration("PT1M30S"), Some(90.0));
    }
}
