//! FFmpeg / ffprobe sidecar resolution and process spawn.
//!
//! Resolution order (D017):
//! 1. `STREAMCRAFT_FFMPEG` / `STREAMCRAFT_FFPROBE` env vars (legacy `OPENSTREAM_*` accepted)
//! 2. Bundled helpers next to the executable (`Helpers/` or `resources/ffmpeg/{os-arch}/`)
//! 3. On-demand install in the user data dir (`install::install_dir`)
//! 4. `PATH`

pub mod install;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum FFmpegError {
    #[error("ffmpeg binary not found")]
    NotFound,
    #[error("ffmpeg exited with status {status}: {stderr}")]
    Failed { status: i32, stderr: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct FFmpegPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub source: String,
}

impl FFmpegPaths {
    pub fn resolve() -> Result<Self, FFmpegError> {
        // Prefer STREAMCRAFT_*; accept legacy OPENSTREAM_* for one release.
        let env_ffmpeg = std::env::var("STREAMCRAFT_FFMPEG")
            .or_else(|_| std::env::var("OPENSTREAM_FFMPEG"));
        let env_ffprobe = std::env::var("STREAMCRAFT_FFPROBE")
            .or_else(|_| std::env::var("OPENSTREAM_FFPROBE"));
        if let (Ok(ffmpeg), Ok(ffprobe)) = (env_ffmpeg, env_ffprobe) {
            let ffmpeg = PathBuf::from(ffmpeg);
            let ffprobe = PathBuf::from(ffprobe);
            if ffmpeg.is_file() && ffprobe.is_file() {
                return Ok(Self {
                    ffmpeg,
                    ffprobe,
                    source: "env".into(),
                });
            }
        }

        if let Some(paths) = find_bundled() {
            return Ok(paths);
        }

        if let Some(dir) = install::install_dir() {
            let (ffmpeg, ffprobe) = (dir.join(exe_name("ffmpeg")), dir.join(exe_name("ffprobe")));
            if ffmpeg.is_file() && ffprobe.is_file() {
                return Ok(Self {
                    ffmpeg,
                    ffprobe,
                    source: format!("installed:{}", dir.display()),
                });
            }
        }

        if let (Some(ffmpeg), Some(ffprobe)) = (which("ffmpeg"), which("ffprobe")) {
            return Ok(Self {
                ffmpeg,
                ffprobe,
                source: "path".into(),
            });
        }

        Err(FFmpegError::NotFound)
    }

    pub async fn run_ffmpeg(&self, args: &[String]) -> Result<(), FFmpegError> {
        debug!(?args, "spawning ffmpeg");
        let output = Command::new(&self.ffmpeg)
            // No version banner: keeps the error surfaced to the user readable.
            .arg("-hide_banner")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(FFmpegError::Failed {
            status: output.status.code().unwrap_or(-1),
            stderr,
        })
    }

    pub async fn probe_duration_seconds(&self, path: &Path) -> Result<Option<f64>, FFmpegError> {
        let output = Command::new(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                &path.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(text.parse::<f64>().ok())
    }

    pub async fn probe_stream_timing(&self, path: &Path) -> Result<MediaStreamTiming, FFmpegError> {
        let output = Command::new(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,start_time,duration,nb_frames,time_base,r_frame_rate,avg_frame_rate",
                "-of",
                "json",
                &path.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if !output.status.success() {
            return Ok(MediaStreamTiming::default());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
        let mut timing = MediaStreamTiming::default();
        if let Some(streams) = parsed.get("streams").and_then(|s| s.as_array()) {
            for stream in streams {
                let codec = stream.get("codec_type").and_then(|v| v.as_str()).unwrap_or("");
                let start = json_f64(stream.get("start_time"));
                let duration = json_f64(stream.get("duration"));
                if codec == "video" && timing.video_start.is_none() {
                    timing.video_start = start;
                    timing.video_duration = duration;
                    timing.video_frame_count = json_f64(stream.get("nb_frames"));
                    timing.video_r_frame_rate = stream
                        .get("r_frame_rate")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                } else if codec == "audio" && timing.audio_start.is_none() {
                    timing.audio_start = start;
                    timing.audio_duration = duration;
                }
            }
        }
        Ok(timing)
    }
}

fn json_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    let v = v?;
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return s.parse().ok();
    }
    None
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MediaStreamTiming {
    pub video_start: Option<f64>,
    pub audio_start: Option<f64>,
    pub video_duration: Option<f64>,
    pub audio_duration: Option<f64>,
    pub video_frame_count: Option<f64>,
    pub video_r_frame_rate: Option<String>,
}

impl MediaStreamTiming {
    pub fn duration_delta(&self) -> Option<f64> {
        Some(self.audio_duration? - self.video_duration?)
    }

    pub fn atempo_to_match_video(&self) -> Option<f64> {
        let v = self.video_duration.filter(|d| *d > 0.5)?;
        let a = self.audio_duration.filter(|d| *d > 0.5)?;
        Some(a / v)
    }

    pub fn video_timeline_skew_ratio(&self) -> Option<f64> {
        let video_duration = self.video_duration.filter(|d| *d > 1.0)?;
        let frames = self.video_frame_count.filter(|f| *f > 0.0)?;
        let fps = nominal_fps(parse_rate(self.video_r_frame_rate.as_deref())?);
        if fps <= 0.0 {
            return None;
        }
        let expected = frames / fps;
        if expected <= 1.0 {
            return None;
        }
        Some(video_duration / expected)
    }
}

/// Snap a measured rate to the nearest broadcast rate (within 3 %). After a
/// timeline compaction (`-itsscale`) ffmpeg reports e.g. 25.42 fps for a 25 fps
/// clip; without snapping the skew would still read ≈1.016 and a second
/// "repair" would compress the file again.
fn nominal_fps(fps: f64) -> f64 {
    const STANDARD: [f64; 9] = [23.976, 24.0, 25.0, 29.97, 30.0, 48.0, 50.0, 59.94, 60.0];
    STANDARD
        .iter()
        .copied()
        .min_by(|a, b| (a - fps).abs().total_cmp(&(b - fps).abs()))
        .filter(|s| (s - fps).abs() / s < 0.03)
        .unwrap_or(fps)
}

fn parse_rate(value: Option<&str>) -> Option<f64> {
    let value = value.filter(|s| !s.is_empty() && *s != "0/0")?;
    if let Some((n, d)) = value.split_once('/') {
        let n: f64 = n.parse().ok()?;
        let d: f64 = d.parse().ok()?;
        if d == 0.0 {
            return None;
        }
        Some(n / d)
    } else {
        value.parse().ok()
    }
}

fn find_bundled() -> Option<FFmpegPaths> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("Helpers"));
            candidates.push(dir.join("resources").join("ffmpeg"));
            candidates.push(dir.join("ffmpeg"));
            let mut cursor = dir.to_path_buf();
            for _ in 0..6 {
                // `Contents/Helpers` when the exe is `StreamCraft.app/Contents/MacOS/…`
                // (where package-macos-app.sh puts them).
                candidates.push(cursor.join("Helpers"));
                candidates.push(cursor.join("resources").join("ffmpeg").join(os_arch()));
                candidates.push(cursor.join("resources").join("Helpers"));
                if !cursor.pop() {
                    break;
                }
            }
        }
    }

    for dir in candidates {
        #[cfg(not(windows))]
        let (ffmpeg, ffprobe) = (dir.join("ffmpeg"), dir.join("ffprobe"));
        #[cfg(windows)]
        let (ffmpeg, ffprobe) = (dir.join("ffmpeg.exe"), dir.join("ffprobe.exe"));

        if ffmpeg.is_file() && ffprobe.is_file() {
            debug!(?dir, "found bundled ffmpeg");
            return Some(FFmpegPaths {
                ffmpeg,
                ffprobe,
                source: format!("bundled:{}", dir.display()),
            });
        }
    }
    None
}

/// `name` plus `.exe` on Windows.
pub(crate) fn exe_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

pub(crate) fn os_arch() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!("{os}-{arch}")
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        #[cfg(not(windows))]
        let candidate = dir.join(name);
        #[cfg(windows)]
        let candidate = dir.join(format!("{name}.exe"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    warn!(%name, "binary not on PATH");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_snaps_to_broadcast_rate() {
        assert_eq!(nominal_fps(305.0 / 12.0), 25.0);
        assert_eq!(nominal_fps(30000.0 / 1001.0), 29.97);
        assert_eq!(nominal_fps(12.5), 12.5);
    }

    #[test]
    fn os_arch_non_empty() {
        assert!(!os_arch().is_empty());
    }
}
