use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::media::export_naming::ExportNamingPreset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    System,
    Http,
    Socks5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VpnIndicatorFilter {
    All,
    #[default]
    IgnoreTailscale,
    NordVpnOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPreferences {
    pub export_dir: PathBuf,
    /// Segments HLS/DASH en parallèle (Swift maxConcurrentSegments).
    pub max_concurrency: usize,
    /// Jobs simultanés (Swift maxConcurrentJobs) — file séquentielle MVP si 1.
    #[serde(default = "default_max_jobs")]
    pub max_concurrent_jobs: usize,
    #[serde(default = "default_retries")]
    pub max_retries: usize,
    pub naming_preset: ExportNamingPreset,
    #[serde(default)]
    pub naming_template: Option<String>,
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    /// Legacy single URL; still honored if proxy_mode=system and set.
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub proxy_host: String,
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
    #[serde(default)]
    pub proxy_username: String,
    #[serde(default)]
    pub proxy_password: String,
    pub ask_quality: bool,
    pub block_popups: bool,
    pub adblock: bool,
    #[serde(default = "default_true")]
    pub block_redirects: bool,
    pub drm_gate: bool,
    #[serde(default)]
    pub vpn_indicator_filter: VpnIndicatorFilter,
    #[serde(default)]
    pub auto_enqueue_hls: bool,
    #[serde(default)]
    pub auto_enqueue_dash: bool,
    #[serde(default)]
    pub auto_enqueue_progressive: bool,
}

fn default_max_jobs() -> usize { 2 }
fn default_retries() -> usize { 3 }
fn default_proxy_port() -> u16 { 8080 }
fn default_true() -> bool { true }

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            export_dir: default_export_dir(),
            max_concurrency: 16,
            max_concurrent_jobs: 2,
            max_retries: 3,
            naming_preset: ExportNamingPreset::JellyfinMovie,
            naming_template: None,
            proxy_mode: ProxyMode::System,
            proxy_url: None,
            proxy_host: String::new(),
            proxy_port: 8080,
            proxy_username: String::new(),
            proxy_password: String::new(),
            ask_quality: false,
            block_popups: true,
            adblock: true,
            block_redirects: true,
            drm_gate: false,
            vpn_indicator_filter: VpnIndicatorFilter::IgnoreTailscale,
            auto_enqueue_hls: false,
            auto_enqueue_dash: false,
            auto_enqueue_progressive: false,
        }
    }
}

impl AppPreferences {
    pub fn should_auto_enqueue(&self, kind: crate::media::models::ManifestKind) -> bool {
        use crate::media::models::ManifestKind::*;
        match kind {
            Hls => self.auto_enqueue_hls,
            Dash => self.auto_enqueue_dash,
            Progressive => self.auto_enqueue_progressive,
        }
    }
}

pub fn default_export_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("StreamCraft")
}

impl AppPreferences {
    pub fn load() -> Self {
        let path = prefs_path();
        let mut prefs: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if let Ok(dir) = std::env::var("STREAMCRAFT_EXPORT_DIR") {
            if !dir.is_empty() {
                prefs.export_dir = PathBuf::from(dir);
            }
        }
        prefs
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = prefs_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap())
    }

    pub fn resolved_naming_template(&self) -> String {
        if self.naming_preset == ExportNamingPreset::Custom {
            self.naming_template
                .clone()
                .unwrap_or_else(|| ExportNamingPreset::Custom.template().to_string())
        } else {
            self.naming_preset.template().to_string()
        }
    }

    /// Build reqwest proxy URL when not using system proxy.
    pub fn explicit_proxy_url(&self) -> Option<String> {
        match self.proxy_mode {
            ProxyMode::System => self.proxy_url.clone().filter(|s| !s.is_empty()),
            ProxyMode::Http | ProxyMode::Socks5 => {
                let host = self.proxy_host.trim();
                if host.is_empty() || self.proxy_port == 0 {
                    return None;
                }
                let scheme = match self.proxy_mode {
                    ProxyMode::Http => "http",
                    ProxyMode::Socks5 => "socks5",
                    ProxyMode::System => unreachable!(),
                };
                if !self.proxy_username.is_empty() {
                    Some(format!(
                        "{scheme}://{}:{}@{host}:{}",
                        urlencoding_simple(&self.proxy_username),
                        urlencoding_simple(&self.proxy_password),
                        self.proxy_port
                    ))
                } else {
                    Some(format!("{scheme}://{host}:{}", self.proxy_port))
                }
            }
        }
    }
}

fn urlencoding_simple(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

fn prefs_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("StreamCraft")
        .join("preferences.json")
}
