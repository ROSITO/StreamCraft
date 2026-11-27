//! On-demand FFmpeg install when no bundled / PATH copy exists.
//!
//! Only runs when the user asks (GUI button, `streamcraft doctor --install-ffmpeg`).
//! Every archive is checked against the SHA-256 its publisher lists before it is
//! unpacked, then the binaries must answer `-version`. Archives are unpacked
//! with the OS's own tools (`ditto` / `tar`), no extra crates.
//!
//! Sources (same as `scripts/fetch-ffmpeg.sh`):
//! - macOS: ffmpeg.martin-riedl.de release channel; `<file>.zip.sha256` next to each zip.
//! - Linux / Windows: BtbN FFmpeg-Builds, LGPL static builds; `checksums.sha256`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::process::Command;

use crate::{FFmpegPaths, exe_name};

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("FFmpeg auto-install is not available for {0}")]
    Unsupported(String),
    #[error("download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("{0}")]
    Message(String),
}

const BTBN: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest";
const RIEDL: &str = "https://ffmpeg.martin-riedl.de/redirect/latest/macos";

/// Where installed binaries live: `<data dir>/StreamCraft/ffmpeg/<os>-<arch>/`.
pub fn install_dir() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("StreamCraft")
            .join("ffmpeg")
            .join(crate::os_arch()),
    )
}

/// One downloadable archive and how to verify it.
struct Archive {
    url: String,
    file_name: String,
    checksum: ChecksumSource,
}

enum ChecksumSource {
    /// `<url>.sha256`, resolved after redirects (Martin Riedl).
    Sibling,
    /// A `sha256sum`-format list; pick the line for `file_name` (BtbN).
    List(String),
}

fn archives() -> Result<Vec<Archive>, InstallError> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let btbn = |asset: &str| Archive {
        url: format!("{BTBN}/{asset}"),
        file_name: asset.to_string(),
        checksum: ChecksumSource::List(format!("{BTBN}/checksums.sha256")),
    };
    Ok(match (os, arch) {
        ("macos", "aarch64" | "x86_64") => {
            let a = if arch == "aarch64" { "arm64" } else { "amd64" };
            ["ffmpeg", "ffprobe"]
                .iter()
                .map(|n| Archive {
                    url: format!("{RIEDL}/{a}/release/{n}.zip"),
                    file_name: format!("{n}.zip"),
                    checksum: ChecksumSource::Sibling,
                })
                .collect()
        }
        ("linux", "x86_64") => vec![btbn("ffmpeg-master-latest-linux64-lgpl.tar.xz")],
        ("linux", "aarch64") => vec![btbn("ffmpeg-master-latest-linuxarm64-lgpl.tar.xz")],
        ("windows", "x86_64") => vec![btbn("ffmpeg-master-latest-win64-lgpl.zip")],
        _ => return Err(InstallError::Unsupported(format!("{os}-{arch}"))),
    })
}

/// First hex token of the line naming `file` (or of the only token, for a bare `.sha256`).
fn parse_checksum(text: &str, file: Option<&str>) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let named = parts.next().map(|n| n.trim_start_matches('*'));
        let ok = match (file, named) {
            (Some(f), Some(n)) => n == f,
            (Some(_), None) | (None, _) => true,
        };
        (ok && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download, verify, unpack and check FFmpeg; returns the resolved paths.
/// `progress` receives short human messages.
pub async fn install(progress: impl Fn(&str)) -> Result<FFmpegPaths, InstallError> {
    let dest = install_dir().ok_or_else(|| InstallError::Message("no data directory".into()))?;
    let work = dest.with_file_name(format!(".install-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&work).await;
    tokio::fs::create_dir_all(&work).await?;
    let result = install_into(&work, &dest, &progress).await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    result
}

async fn install_into(
    work: &Path,
    dest: &Path,
    progress: &impl Fn(&str),
) -> Result<FFmpegPaths, InstallError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("StreamCraft/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let unpacked = work.join("unpacked");
    tokio::fs::create_dir_all(&unpacked).await?;

    for a in archives()? {
        progress(&format!("Téléchargement de {}…", a.file_name));
        let resp = client.get(&a.url).send().await?.error_for_status()?;
        let final_url = resp.url().clone();
        let bytes = resp.bytes().await?;

        let expected = match &a.checksum {
            ChecksumSource::Sibling => {
                let url = format!("{final_url}.sha256");
                let text = client.get(url).send().await?.error_for_status()?.text().await?;
                parse_checksum(&text, None)
            }
            ChecksumSource::List(url) => {
                let text = client.get(url).send().await?.error_for_status()?.text().await?;
                parse_checksum(&text, Some(&a.file_name))
            }
        }
        .ok_or_else(|| InstallError::Message(format!("no published checksum for {}", a.file_name)))?;
        let actual = sha256_hex(&bytes);
        if actual != expected {
            return Err(InstallError::Checksum {
                file: a.file_name,
                expected,
                actual,
            });
        }
        progress(&format!("{} vérifié (SHA-256)", a.file_name));

        let archive = work.join(&a.file_name);
        tokio::fs::write(&archive, &bytes).await?;
        unpack(&archive, &unpacked).await?;
    }

    progress("Installation…");
    tokio::fs::create_dir_all(dest).await?;
    for name in ["ffmpeg", "ffprobe"] {
        let file = exe_name(name);
        let found = find_file(&unpacked, &file)
            .ok_or_else(|| InstallError::Message(format!("{file} missing from archive")))?;
        let target = dest.join(&file);
        let _ = tokio::fs::remove_file(&target).await;
        tokio::fs::copy(&found, &target).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).await?;
        }
        let ok = Command::new(&target)
            .arg("-version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            let _ = tokio::fs::remove_file(&target).await;
            return Err(InstallError::Message(format!("{file} does not run on this system")));
        }
    }
    FFmpegPaths::resolve().map_err(|e| InstallError::Message(e.to_string()))
}

async fn unpack(archive: &Path, into: &Path) -> Result<(), InstallError> {
    let name = archive.to_string_lossy();
    let mut cmd = if cfg!(target_os = "macos") && name.ends_with(".zip") {
        let mut c = Command::new("ditto");
        c.arg("-x").arg("-k").arg(archive).arg(into);
        c
    } else {
        // GNU tar (xz) on Linux; bsdtar on Windows 10+ also reads zip.
        let mut c = Command::new("tar");
        c.arg("-xf").arg(archive).arg("-C").arg(into);
        c
    };
    let out = cmd.output().await?;
    if !out.status.success() {
        return Err(InstallError::Message(format!(
            "unpacking {} failed: {}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_lines_are_matched_by_file_name() {
        let list = "aaaa  other\n23b53365b9b7ee25d5bd89f4d24733d1100fda30877c4830f43546717abbeae4  ffmpeg-master-latest-linux64-lgpl.tar.xz\n";
        assert_eq!(
            parse_checksum(list, Some("ffmpeg-master-latest-linux64-lgpl.tar.xz")).as_deref(),
            Some("23b53365b9b7ee25d5bd89f4d24733d1100fda30877c4830f43546717abbeae4")
        );
        assert_eq!(parse_checksum(list, Some("missing.zip")), None);
        let bare = "23B53365B9B7EE25D5BD89F4D24733D1100FDA30877C4830F43546717ABBEAE4\n";
        assert!(parse_checksum(bare, None).is_some());
    }

    #[test]
    fn sha256_of_known_input() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
