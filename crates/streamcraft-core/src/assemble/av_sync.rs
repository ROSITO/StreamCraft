//! AV sync repairs — ports Swift MediaAssembler realign / drift / manual shift.

use std::path::{Path, PathBuf};

use streamcraft_ffmpeg::{FFmpegPaths, MediaStreamTiming};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

pub const AV_SYNC_TOLERANCE_SECONDS: f64 = 0.04;
pub const AV_DURATION_MISMATCH_LIMIT_SECONDS: f64 = 1.0;

#[derive(Debug, Error)]
pub enum AvSyncError {
    #[error(transparent)]
    Ffmpeg(#[from] streamcraft_ffmpeg::FFmpegError),
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
    /// Start offset between tracks removed (Swift `repaired`).
    Repaired { before_delta_seconds: f64, after_delta_seconds: f64 },
    /// Stretched video timeline compacted, audio sped up by the same ratio.
    TimelineCorrected { skew_ratio: f64 },
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
            Self::Repaired {
                before_delta_seconds,
                ..
            } => format!(
                "Sync corrigé (écart {:.0} ms → aligné)",
                before_delta_seconds * 1000.0
            ),
            Self::TimelineCorrected { skew_ratio } => format!(
                "Dérive corrigée (audio ×{skew_ratio:.4} pour coller à la timeline)"
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
    // Swift: compact a stretched video timeline first (PTS gaps), then check durations.
    if let Some(skew) = normalize_timeline_skew(ffmpeg, path).await? {
        return Ok(Some(AvSyncResult::TimelineCorrected { skew_ratio: skew }));
    }
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
    // A stretched timeline isn't fixed by trimming: compact it instead (Swift).
    if let Some(skew) = normalize_timeline_skew(ffmpeg, url).await? {
        return Ok(AvSyncResult::TimelineCorrected { skew_ratio: skew });
    }
    let trimmed = (before.video_duration.unwrap_or(0.0) - before.audio_duration.unwrap_or(0.0)).abs();

    let staging = transient_path(url, "player_sync");
    let video_part = transient_path(url, "repair_v");
    let audio_part = transient_path(url, "repair_a");
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
    scrub_transients(url).await;

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

    let staging = transient_path(url, "drift_sync");
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
    scrub_transients(url).await;
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
    let staging = transient_path(url, "shift_sync");
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
    scrub_transients(url).await;
    Ok(AvSyncResult::OffsetApplied {
        audio_shift_seconds: shift,
    })
}

/// Swift `repairAVSync`: a stretched timeline is compacted; otherwise a constant
/// start offset between tracks is removed by delaying the track that starts early.
pub async fn repair_av_sync(ffmpeg: &FFmpegPaths, url: &Path) -> Result<AvSyncResult, AvSyncError> {
    if !url.is_file() {
        return Err(AvSyncError::Message("Fichier introuvable".into()));
    }
    let before = probe(ffmpeg, url).await?;
    let (Some(v_start), Some(a_start)) = (before.video_start, before.audio_start) else {
        return Ok(AvSyncResult::MissingAudioOrVideo);
    };
    if let Some(skew) = normalize_timeline_skew(ffmpeg, url).await? {
        return Ok(AvSyncResult::TimelineCorrected { skew_ratio: skew });
    }
    let before_delta = (v_start - a_start).abs();
    if before_delta <= AV_SYNC_TOLERANCE_SECONDS {
        return Ok(AvSyncResult::AlreadyAligned { delta_seconds: before_delta });
    }
    // > 0: audio starts too early → delay it.
    let result = apply_manual_audio_shift(ffmpeg, url, v_start - a_start).await?;
    if !matches!(result, AvSyncResult::OffsetApplied { .. }) {
        return Ok(result);
    }
    let after = probe(ffmpeg, url).await?;
    let after_delta = match (after.video_start, after.audio_start) {
        (Some(v), Some(a)) => (v - a).abs(),
        _ => 0.0,
    };
    Ok(AvSyncResult::Repaired {
        before_delta_seconds: before_delta,
        after_delta_seconds: after_delta,
    })
}

/// Video timeline stretched by PTS gaps if duration / (frames / fps) is off by
/// more than 0.2 %; beyond 8 % the export is broken, not stretched.
fn needs_skew_fix(skew: f64) -> bool {
    skew > 1.002 && skew < 1.08
}

/// Swift `normalizeTimelineSkewIfNeeded`: compress video timestamps by 1/skew
/// (`-itsscale`, no re-encode) and speed audio up by the same ratio.
/// Returns the ratio when a fix was applied.
pub async fn normalize_timeline_skew(
    ffmpeg: &FFmpegPaths,
    url: &Path,
) -> Result<Option<f64>, AvSyncError> {
    let timing = probe(ffmpeg, url).await?;
    let Some(skew) = timing.video_timeline_skew_ratio().filter(|s| needs_skew_fix(*s)) else {
        return Ok(None);
    };
    info!(skew, "stretched video timeline — compacting");
    let video_part = transient_path(url, "skew_v");
    let staging = transient_path(url, "skew_fix");
    let _ = tokio::fs::remove_file(&video_part).await;
    let _ = tokio::fs::remove_file(&staging).await;
    let result = async {
        ffmpeg
            .run_ffmpeg(&[
                "-y".into(),
                "-itsscale".into(),
                format!("{:.8}", 1.0 / skew),
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
                video_part.display().to_string(),
                "-i".into(),
                url.display().to_string(),
                "-map".into(),
                "0:v:0".into(),
                "-map".into(),
                "1:a:0".into(),
                "-c:v".into(),
                "copy".into(),
                "-af".into(),
                atempo_filter_chain(skew),
                "-c:a".into(),
                "aac".into(),
                "-movflags".into(),
                "+faststart".into(),
                "-shortest".into(),
                staging.display().to_string(),
            ])
            .await?;
        replace_atomically(url, &staging).await
    }
    .await;
    let _ = tokio::fs::remove_file(&video_part).await;
    let _ = tokio::fs::remove_file(&staging).await;
    result.map(|()| Some(skew))
}

async fn mux_aligned(
    ffmpeg: &FFmpegPaths,
    video: &Path,
    audio: &Path,
    output: &Path,
) -> Result<(), AvSyncError> {
    // Swift muxAlignedAudioVideo: one remux, -copyts + -start_at_zero (single global
    // offset) + -shortest (player-like trim to the shorter track).
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
        "-start_at_zero".into(),
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

/// Scratch file next to `media`, unique per output file: concurrent jobs
/// exporting into the same folder must not share (or delete) each other's temps.
fn transient_path(media: &Path, tag: &str) -> PathBuf {
    let name = media.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    media
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".__os_{tag}_{name}"))
}

/// Remove leftover temps belonging to `media` only.
async fn scrub_transients(media: &Path) {
    let work = media.parent().unwrap_or(Path::new("."));
    let Some(own) = media.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return;
    };
    if let Ok(rd) = tokio::fs::read_dir(work).await {
        let mut rd = rd;
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".__os_") && name.ends_with(&format!("_{own}")) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transients_are_per_output_file() {
        let a = transient_path(Path::new("/x/Film A (2020).mp4"), "repair_v");
        let b = transient_path(Path::new("/x/Film B (2021).mp4"), "repair_v");
        assert_ne!(a, b);
        assert_eq!(a, Path::new("/x/.__os_repair_v_Film A (2020).mp4"));
    }

    /// Builds a real 10 s clip with the bundled/PATH ffmpeg; skipped if absent.
    async fn clip(dir: &Path) -> Option<(FFmpegPaths, PathBuf)> {
        let ff = FFmpegPaths::resolve().ok()?;
        let src = dir.join("src.mp4");
        ff.run_ffmpeg(&[
            "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=10",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=10",
            "-c:v", "libx264", "-g", "50", "-c:a", "aac", "-shortest",
        ].iter().map(|s| s.to_string()).chain([src.display().to_string()]).collect::<Vec<_>>())
        .await
        .ok()?;
        Some((ff, src))
    }

    fn tmp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-avsync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn stretched_timeline_is_compacted() {
        let dir = tmp_dir();
        let Some((ff, src)) = clip(&dir).await else { return };
        let skewed = dir.join("skewed.mp4");
        // PTS gaps: drop 1 frame in 50 without retiming → duration / (frames / fps) ≈ 1.016.
        ff.run_ffmpeg(&[
            "-y".into(), "-i".into(), src.display().to_string(),
            "-vf".into(), "select='not(eq(mod(n,50),0))'".into(), "-fps_mode".into(), "passthrough".into(),
            "-c:v".into(), "libx264".into(), "-c:a".into(), "copy".into(), skewed.display().to_string(),
        ])
        .await
        .unwrap();
        let before = probe(&ff, &skewed).await.unwrap().video_timeline_skew_ratio().unwrap();
        assert!(before > 1.01 && before < 1.03, "fixture skew {before}");
        let fixed = normalize_timeline_skew(&ff, &skewed).await.unwrap();
        assert!(fixed.is_some());
        let t = probe(&ff, &skewed).await.unwrap();
        let after = t.video_timeline_skew_ratio().unwrap();
        assert!((after - 1.0).abs() < 0.002, "after skew {after}");
        // Video now lasts frames / 25 fps; audio sped up by the same ratio.
        let frames = t.video_frame_count.unwrap();
        assert!((t.video_duration.unwrap() - frames / 25.0).abs() < 0.05);
        assert!((t.audio_duration.unwrap() - 10.0 / before).abs() < 0.1, "audio {:?}", t.audio_duration);
        // Idempotent: a second pass must not compress again.
        assert!(normalize_timeline_skew(&ff, &skewed).await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn start_offset_is_repaired() {
        let dir = tmp_dir();
        let Some((ff, src)) = clip(&dir).await else { return };
        let off = dir.join("offset.mp4");
        // Audio starts 0.5 s after video.
        ff.run_ffmpeg(&[
            "-y".into(), "-i".into(), src.display().to_string(), "-itsoffset".into(), "0.5".into(),
            "-i".into(), src.display().to_string(), "-map".into(), "0:v".into(), "-map".into(), "1:a".into(),
            "-c".into(), "copy".into(), off.display().to_string(),
        ])
        .await
        .unwrap();
        let r = repair_av_sync(&ff, &off).await.unwrap();
        let AvSyncResult::Repaired { before_delta_seconds, after_delta_seconds } = r else {
            panic!("unexpected {r:?}");
        };
        assert!(before_delta_seconds > 0.4, "before {before_delta_seconds}");
        assert!(after_delta_seconds < 0.05, "after {after_delta_seconds}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skew_window_matches_swift() {
        assert!(!needs_skew_fix(1.001));
        assert!(needs_skew_fix(1.015));
        assert!(!needs_skew_fix(1.2));
    }
}
