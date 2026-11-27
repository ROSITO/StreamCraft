use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportNamingPreset {
    Flat,
    JellyfinMovie,
    JellyfinSeries,
    Custom,
}

impl ExportNamingPreset {
    pub fn template(self) -> &'static str {
        match self {
            Self::Flat => "{title}.{ext}",
            Self::JellyfinMovie => "{title} ({year})/{title}.{ext}",
            Self::JellyfinSeries => "{show}/Saison {season}/S{season}E{episode}.{ext}",
            Self::Custom => "{title} ({year})/{title}.{ext}",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Flat => "Fichier plat",
            Self::JellyfinMovie => "Film",
            Self::JellyfinSeries => "Série",
            Self::Custom => "Personnalisé",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Flat => "Un seul fichier : Titre.mp4 dans le dossier d’export.",
            Self::JellyfinMovie => "Film (2020)/Film.mp4 — dossier = nom + année, fichier = nom.",
            Self::JellyfinSeries => "Série/Saison 01/S01E01.mp4 — renseignez série, saison et épisode.",
            Self::Custom => "Tokens : {title} {year} {show} {season} {episode} {episode_title} {kind} {ext}",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportMetadata {
    pub title: String,
    pub year: Option<String>,
    pub show: Option<String>,
    pub season: Option<String>,
    pub episode: Option<String>,
    pub episode_title: Option<String>,
    pub naming_preset: Option<ExportNamingPreset>,
}

impl ExportMetadata {
    pub fn into_context(self, kind: &str, ext: &str) -> ExportNamingContext {
        ExportNamingContext {
            title: if self.title.trim().is_empty() {
                "video".into()
            } else {
                self.title
            },
            year: self.year.filter(|s| !s.trim().is_empty()),
            show: self.show.filter(|s| !s.trim().is_empty()),
            season: self.season.filter(|s| !s.trim().is_empty()),
            episode: self.episode.filter(|s| !s.trim().is_empty()),
            episode_title: self.episode_title.filter(|s| !s.trim().is_empty()),
            kind: kind.into(),
            ext: ext.into(),
        }
    }
}

pub struct ExportNamingContext {
    pub title: String,
    pub year: Option<String>,
    pub show: Option<String>,
    pub season: Option<String>,
    pub episode: Option<String>,
    pub episode_title: Option<String>,
    pub kind: String,
    pub ext: String,
}

pub struct ExportNaming;

impl ExportNaming {
    pub fn relative_path(template: &str, context: &ExportNamingContext) -> String {
        let values = [
            ("title", sanitize_segment(&context.title, false)),
            (
                "year",
                sanitize_segment(context.year.as_deref().unwrap_or(""), true),
            ),
            (
                "show",
                sanitize_segment(
                    context.show.as_deref().unwrap_or(&context.title),
                    false,
                ),
            ),
            (
                "season",
                sanitize_segment(&padded_season_episode(context.season.as_deref()), true),
            ),
            (
                "episode",
                sanitize_segment(&padded_season_episode(context.episode.as_deref()), true),
            ),
            (
                "episode_title",
                sanitize_segment(context.episode_title.as_deref().unwrap_or(""), true),
            ),
            ("kind", sanitize_segment(&context.kind, false)),
            ("ext", sanitize_extension(&context.ext)),
        ];
        let mut result = template.to_string();
        for (key, value) in values {
            result = result.replace(&format!("{{{key}}}"), &value);
        }
        result = cleanup_empty_tokens(&result);
        let parts: Vec<&str> = result
            .split('/')
            .map(str::trim)
            .filter(|p| !p.is_empty() && *p != "." && *p != "..")
            .collect();
        let mut result = parts.join("/");
        if result.is_empty() {
            return format!(
                "{}.{}",
                sanitize_segment(&context.title, false),
                sanitize_extension(&context.ext)
            );
        }
        if Path::new(&result).extension().is_none() {
            result.push('.');
            result.push_str(&sanitize_extension(&context.ext));
        }
        result
    }

    pub fn parse_title_and_year(raw: &str) -> (String, Option<String>) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return ("video".into(), None);
        }
        let re1 = Regex::new(r"^(.*?)[\s\-_]*\((\d{4})\)(.*)$").unwrap();
        if let Some(caps) = re1.captures(trimmed) {
            let mut title = caps.get(1).unwrap().as_str().trim().to_string();
            title = title
                .trim_matches(|c: char| "-–—|_".contains(c) || c.is_whitespace())
                .to_string();
            let year = caps.get(2).unwrap().as_str().to_string();
            if title.is_empty() {
                title = "video".into();
            }
            return (title, Some(year));
        }
        let re2 = Regex::new(r"^(.*?)[\s\-_]*((?:19|20)\d{2})\s*$").unwrap();
        if let Some(caps) = re2.captures(trimmed) {
            let mut title = caps.get(1).unwrap().as_str().trim().to_string();
            title = title
                .trim_matches(|c: char| "-–—|_".contains(c) || c.is_whitespace())
                .to_string();
            let year = caps.get(2).unwrap().as_str().to_string();
            if title.is_empty() {
                title = "video".into();
            }
            return (title, Some(year));
        }
        (trimmed.to_string(), None)
    }

    pub fn unique_path(directory: &Path, preferred_name: &str) -> PathBuf {
        let _ = std::fs::create_dir_all(directory);
        let candidate = directory.join(preferred_name);
        if !candidate.exists() {
            return candidate;
        }
        let stem = Path::new(preferred_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("export");
        let ext = Path::new(preferred_name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("mp4");
        for i in 2..=99 {
            let c = directory.join(format!("{stem}-{i}.{ext}"));
            if !c.exists() {
                return c;
            }
        }
        directory.join(format!("{stem}-{}.{}", uuid::Uuid::new_v4().simple(), ext))
    }
}

fn sanitize_segment(name: &str, allow_empty: bool) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "/:\\?%*|\"<>".contains(c) { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        if allow_empty {
            String::new()
        } else {
            "video".into()
        }
    } else {
        cleaned.chars().take(120).collect()
    }
}

fn sanitize_extension(ext: &str) -> String {
    let cleaned: String = ext
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if cleaned.is_empty() {
        "mp4".into()
    } else {
        cleaned
    }
}

/// First number in the field, zero-padded: "4", "04", "S4", "saison 4" → "04".
/// Free text with no digit is kept as typed.
fn padded_season_episode(value: Option<&str>) -> String {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return String::new();
    };
    let digits: String = v
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.parse::<u32>() {
        Ok(n) => format!("{n:02}"),
        Err(_) => v.to_string(),
    }
}

fn cleanup_empty_tokens(path: &str) -> String {
    let mut result = path.replace(" ()", "").replace("()", "");
    result = result.replace("Season /", "");
    result = result.replace(" - SE", "").replace(" - S E", "");
    while result.contains("  ") {
        result = result.replace("  ", " ");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_template() {
        let ctx = ExportNamingContext {
            title: "My Film".into(),
            year: Some("2020".into()),
            show: None,
            season: None,
            episode: None,
            episode_title: None,
            kind: "mp4".into(),
            ext: "mp4".into(),
        };
        assert_eq!(
            ExportNaming::relative_path(ExportNamingPreset::Flat.template(), &ctx),
            "My Film.mp4"
        );
    }

    #[test]
    fn season_episode_accept_words_around_the_number() {
        for (raw, want) in [("4", "04"), ("04", "04"), ("saison 4", "04"), ("S4", "04"), ("E07", "07"), ("pilot", "pilot")] {
            assert_eq!(padded_season_episode(Some(raw)), want, "{raw}");
        }
    }
}
