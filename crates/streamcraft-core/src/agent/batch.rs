//! BatchURLParser — port Swift `ProductivitySupport.BatchURLParser`.

use url::Url;

use crate::media::detector::MediaDetector;
use crate::media::models::MediaClassification;

pub struct BatchPartition {
    pub media: Vec<Url>,
    pub pages: Vec<Url>,
}

pub fn parse(text: &str) -> Vec<Url> {
    let tokens = text
        .replace(',', "\n")
        .split(|c: char| c.is_whitespace())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect::<Vec<_>>();

    let mut seen = std::collections::HashSet::new();
    let mut urls = Vec::new();
    for token in tokens {
        let with_scheme = if token.starts_with("http://") || token.starts_with("https://") {
            token
        } else if token.contains('.') {
            format!("https://{token}")
        } else {
            continue;
        };
        let Ok(url) = Url::parse(&with_scheme) else {
            continue;
        };
        if url.scheme() != "http" && url.scheme() != "https" {
            continue;
        }
        if url.host_str().is_none() {
            continue;
        }
        let key = url.as_str().to_lowercase();
        if !seen.insert(key) {
            continue;
        }
        urls.push(url);
    }
    urls
}

pub fn classify(url: &Url) -> MediaClassification {
    MediaDetector::classify(url, None)
}

pub fn partition(urls: &[Url]) -> BatchPartition {
    let mut media = Vec::new();
    let mut pages = Vec::new();
    for url in urls {
        match classify(url) {
            MediaClassification::Hls
            | MediaClassification::Dash
            | MediaClassification::Progressive => media.push(url.clone()),
            MediaClassification::Segment | MediaClassification::Unknown => pages.push(url.clone()),
        }
    }
    BatchPartition { media, pages }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitions_mixed() {
        let urls = parse(
            "https://cdn.example/a.m3u8\nhttps://site.example/watch/1\nhttps://cdn.example/v.mp4",
        );
        let p = partition(&urls);
        assert_eq!(p.media.len(), 2);
        assert_eq!(p.pages.len(), 1);
    }
}
