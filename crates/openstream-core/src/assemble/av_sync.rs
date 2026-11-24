//! AV sync repairs — ports Swift MediaAssembler realign / drift / manual shift.

use std::path::Path;

use openstream_ffmpeg::{FFmpegPaths, MediaStreamTiming};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

pub const AV_SYNC_TOLERANCE_SECONDS: f64 = 0.04;
pub const AV_DURATION_MISMATCH_LIMIT_SECONDS: f64 = 1.0;

#[derive(Debug, Error)]
pub enum AvSyncError {
    #[error(transparent)]
    Ffmpeg(#[from] openstream_ffmpeg::FFmpegError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvSyncResult {
    AlreadyAligned { delta_seconds: f64 },
    PlayerAligned { trimmed_seconds: f64 },
    DriftCorrected { duration_delta_seconds: f64, atempo: f64 },
    OffsetApplied { audio_shift_seconds: f64 },
    MissingAudioOrVideo,
}

impl AvSyncResult {
    pub fn user_message(&self) -> String {
        match self {
            Self::AlreadyAligned { delta_seconds } => {
                format!("Déjà aligné (Δ {:.0} ms)", delta_seconds * 1000.0)
            }
            Self::PlayerAligned { trimmed_seconds } => format!(
                "Recalé comme le lecteur (rogne {:.0} ms sur la piste longue)",
                trimmed_seconds * 1000.0
            ),
            Self::DriftCorrected {
                duration_delta_seconds,
                atempo,
            } => format!(
                "Dérive corrigée ({:.0} ms, atempo {:.5})",
                duration_delta_seconds * 1000.0,
                atempo
            ),
            Self::OffsetApplied {
                audio_shift_seconds,
            } => format!(
                "Décalage appliqué ({:+.0} ms)",
                audio_shift_seconds * 1000.0
            ),
            Self::MissingAudioOrVideo => "Fichier sans piste audio et vidéo".into(),
        }
    }
}

pub async fn probe(ffmpeg: &FFmpegPaths, path: &Path) -> Result<MediaStreamTiming, AvSyncError> {
    Ok(ffmpeg.probe_stream_timing(path).await?)
}

/// After remux: if |A−V| > 40ms, realign like player; if still extreme, return Err for UX.
pub async fn post_export_fixup(
    ffmpeg: &FFmpegPaths,
    path: &Path,
) -> Result<Option<AvSyncResult>, AvSyncError> {
    let timing = probe(ffmpeg, path).await?;
    let Some(delta) = timing.duration_delta() else {
        return Ok(None);
    };
    let abs = delta.abs();
    if abs <= AV_SYNC_TOLERANCE_SECONDS {
        return Ok(None);
    }
    info!(delta, "AV duration mismatch — realigning like player");
    let result = realign_like_player(ffmpeg, path).await?;
    let after = probe(ffmpeg, path).await?;
    if let Some(d2) = after.duration_delta() {
        if d2.abs() > AV_DURATION_MISMATCH_LIMIT_SECONDS {
            return Err(AvSyncError::Message(format!(
                "Pistes A/V encore désalignées ({:.1}s) après recalage — ouvre Sync… ou re-télécharge",
                d2.abs()
            )));
        }
    }
    Ok(Some(result))
}

/// Coupe à la piste courte + remux (-shortest), comme Swift realignLikePlayer.
pub async fn realign_like_player(
    ffmpeg: &FFmpegPaths,
    url: &Path,
) -> Result<AvSyncResult, AvSyncError> {
    if !url.is_file() {
        return Err(AvSyncError::Message("Fichier introuvable".into()));
    }
    let before = probe(ffmpeg, url).await?;
    if before.video_start.is_none() || before.audio_start.is_none() {
        return Ok(AvSyncResult::MissingAudioOrVideo);
    }
    let trimmed = (before.video_duration.unwrap_or(0.0) - before.audio_duration.unwrap_or(0.0)).abs();

    let work = url.parent().unwrap_or(Path::new("."));
    let staging = work.join(".__os_player_sync.mp4");
    let video_part = work.join(".__os_repair_v.mp4");
    let audio_part = work.join(".__os_repair_a.mp4");
    let _ = tokio::fs::remove_file(&staging).await;
    let _ = tokio::fs::remove_file(&video_part).await;
    let _ = tokio::fs::remove_file(&audio_part).await;

    ffmpeg
        .run_ffmpeg(&[
            "-y".into(),
            "-i".into(),
            url.display().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-c".into(),
            "copy".into(),
            video_part.display().to_string(),
        ])
        .await?;
    ffmpeg
        .run_ffmpeg(&[
            "-y".into(),
            "-i".into(),
            url.display().to_string(),
            "-map".into(),
            "0:a:0".into(),
            "-c".into(),
            "copy".into(),
            audio_part.display().to_string(),
        ])
        .await?;

    mux_aligned(ffmpeg, &video_part, &audio_part, &staging).await?;
    replace_atomically(url, &staging).await?;
    let _ = tokio::fs::remove_file(&video_part).await;
    let _ = tokio::fs::remove_file(&audio_part).await;
    scrub_transients(work).await;

    Ok(AvSyncResult::PlayerAligned {
        trimmed_seconds: trimmed,
    })
}

pub async fn correct_av_drift(
    ffmpeg: &FFmpegPaths,
    url: &Path,
    end_audio_lead_seconds: Option<f64>,
) -> Result<AvSyncResult, AvSyncError> {
    if !url.is_file() {
        return Err(AvSyncError::Message("Fichier introuvable".into()));
    }
    let timing = probe(ffmpeg, url).await?;
    if timing.video_start.is_none() || timing.audio_start.is_none() {
        return Ok(AvSyncResult::MissingAudioOrVideo);
    }
    let video_duration = timing
        .video_duration
        .filter(|d| *d > 1.0)
        .ok_or_else(|| AvSyncError::Message("Durée vidéo illisible".into()))?;

    let (lead, atempo) = if let Some(end_lead) = end_audio_lead_seconds {
        let rounded = (end_lead * 1000.0).round() / 1000.0;
        if rounded.abs() < 0.02 {
            return Ok(AvSyncResult::AlreadyAligned {
                delta_seconds: timing.duration_delta().unwrap_or(0.0).abs(),
            });
        }
        let denom = video_duration + rounded;
        if denom <= 1.0 {
            return Err(AvSyncError::Message("Dérive trop extrême".into()));
        }
        (rounded, video_duration / denom)
    } else {
        let delta = timing
            .duration_delta()
            .ok_or_else(|| AvSyncError::Message("Durées A/V illisibles".into()))?;
        if delta.abs() < 0.05 {
            return Ok(AvSyncResult::AlreadyAligned {
                delta_seconds: delta.abs(),
            });
        }
        let ratio = timing
            .atempo_to_match_video()
            .ok_or_else(|| AvSyncError::Message("Durées A/V illisibles".into()))?;
        (-delta, ratio)
    };

    if !(0.5..=2.0).contains(&atempo) {
        return Err(AvSyncError::Message(format!(
            "Dérive trop extrême (atempo {atempo:.5}) — re-téléchargement conseillé"
        )));
    }

    let work = url.parent().unwrap_or(Path::new("."));
    let staging = work.join(".__os_drift_sync.mp4");
    let _ = tokio::fs::remove_file(&staging).await;
    let filter = atempo_filter_chain(atempo);
    ffmpeg
        .run_ffmpeg(&[
            "-y".into(),
            "-i".into(),
            url.display().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "0:a:0".into(),
            "-c:v".into(),
            "copy".into(),
            "-c:a".into(),
            "aac".into(),
            "-af".into(),
            filter,
            "-shortest".into(),
            "-avoid_negative_ts".into(),
            "make_zero".into(),
            "-movflags".into(),
            "+faststart".into(),
            staging.display().to_string(),
        ])
        .await?;
    replace_atomically(url, &staging).await?;
    scrub_transients(work).await;
    Ok(AvSyncResult::DriftCorrected {
        duration_delta_seconds: lead,
        atempo,
    })
}

pub async fn apply_manual_audio_shift(
    ffmpeg: &FFmpegPaths,
    url: &Path,
    audio_shift_seconds: f64,
) -> Result<AvSyncResult, AvSyncError> {
    let shift = (audio_shift_seconds * 1000.0).round() / 1000.0;
    if shift.abs() < 0.001 {
        return Ok(AvSyncResult::OffsetApplied {
            audio_shift_seconds: 0.0,
        });
    }
    let timing = probe(ffmpeg, url).await?;
    if timing.video_start.is_none() || timing.audio_start.is_none() {
        return Ok(AvSyncResult::MissingAudioOrVideo);
    }
    let work = url.parent().unwrap_or(Path::new("."));
    let staging = work.join(".__os_shift_sync.mp4");
    let _ = tokio::fs::remove_file(&staging).await;

    let mut args: Vec<String> = vec!["-y".into()];
    if shift < 0.0 {
        // Son trop tard → retarder la vidéo
        args.extend([
            "-itsoffset".into(),
            format!("{:.3}", -shift),
            "-i".into(),
            url.display().to_string(),
            "-i".into(),
            url.display().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "1:a:0".into(),
        ]);
    } else {
        args.extend([
            "-i".into(),
            url.display().to_string(),
            "-itsoffset".into(),
            format!("{:.3}", shift),
            "-i".into(),
            url.display().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "1:a:0".into(),
        ]);
    }
    args.extend([
        "-c".into(),
        "copy".into(),
        "-shortest".into(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
        "-muxpreload".into(),
        "0".into(),
        "-muxdelay".into(),
        "0".into(),
        "-movflags".into(),
        "+faststart".into(),
        staging.display().to_string(),
    ]);
    ffmpeg.run_ffmpeg(&args).await?;
    replace_atomically(url, &staging).await?;
    scrub_transients(work).await;
    Ok(AvSyncResult::OffsetApplied {
        audio_shift_seconds: shift,
    })
}

async fn mux_aligned(
    ffmpeg: &FFmpegPaths,
    video: &Path,
    audio: &Path,
    output: &Path,
) -> Result<(), AvSyncError> {
    // Swift: -copyts then -shortest (player-like trim to shorter track)
    let args = vec![
        "-y".into(),
        "-copyts".into(),
        "-i".into(),
        video.display().to_string(),
        "-i".into(),
        audio.display().to_string(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "1:a:0".into(),
        "-c".into(),
        "copy".into(),
        "-shortest".into(),
        "-movflags".into(),
        "+faststart".into(),
        output.display().to_string(),
    ];
    if let Err(e) = ffmpeg.run_ffmpeg(&args).await {
        let args2 = vec![
            "-y".into(),
            "-i".into(),
            video.display().to_string(),
            "-i".into(),
            audio.display().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "1:a:0".into(),
            "-c".into(),
            "copy".into(),
            "-shortest".into(),
            "-movflags".into(),
            "+faststart".into(),
            output.display().to_string(),
        ];
        ffmpeg.run_ffmpeg(&args2).await.map_err(|_| e)?;
    }
    Ok(())
}

async fn replace_atomically(original: &Path, staging: &Path) -> Result<(), AvSyncError> {
    let bak = original.with_extension("mp4.__os_bak");
    let _ = tokio::fs::remove_file(&bak).await;
    tokio::fs::rename(original, &bak).await?;
    if let Err(e) = tokio::fs::rename(staging, original).await {
        let _ = tokio::fs::rename(&bak, original).await;
        return Err(e.into());
    }
    let _ = tokio::fs::remove_file(&bak).await;
    Ok(())
}

async fn scrub_transients(work: &Path) {
    if let Ok(rd) = tokio::fs::read_dir(work).await {
        let mut rd = rd;
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".__os_") {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

fn atempo_filter_chain(ratio: f64) -> String {
    let mut r = ratio;
    let mut parts = Vec::new();
    while r > 2.0 {
        parts.push("atempo=2.0".to_string());
        r /= 2.0;
    }
    while r < 0.5 {
        parts.push("atempo=0.5".to_string());
        r /= 0.5;
    }
    parts.push(format!("atempo={r:.6}"));
    parts.join(",")
}

pub fn mismatch_warning(timing: &MediaStreamTiming) -> Option<String> {
    let delta = timing.duration_delta()?;
    let abs = delta.abs();
    if abs <= AV_SYNC_TOLERANCE_SECONDS {
        return None;
    }
    if abs > AV_DURATION_MISMATCH_LIMIT_SECONDS {
        Some(format!(
            "Écart A/V {:.1}s — segments incomplets possibles. Recaler ou re-télécharger.",
            abs
        ))
    } else {
        Some(format!(
            "Écart A/V {:.0} ms — « Recaler » rogne la piste longue.",
            abs * 1000.0
        ))
    }
}
