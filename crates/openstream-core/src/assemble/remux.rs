//! Assemble downloaded units into MP4 via FFmpeg (-c copy). D013: HLS via local m3u8.

use std::path::Path;

use openstream_ffmpeg::FFmpegPaths;
use thiserror::Error;
use tracing::debug;

use crate::download::planner::DownloadPlan;
use crate::media::models::ManifestKind;

#[derive(Debug, Error)]
pub enum AssembleError {
    #[error(transparent)]
    Ffmpeg(#[from] openstream_ffmpeg::FFmpegError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

pub async fn assemble(
    ffmpeg: &FFmpegPaths,
    plan: &DownloadPlan,
    parts_dir: &Path,
    output: &Path,
) -> Result<(), AssembleError> {
    if let Some(parent) = output.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    match plan.kind {
        ManifestKind::Progressive => {
            let src = parts_dir.join(&plan.units[0].file_name);
            if plan.units[0].file_name.ends_with(".mp4") {
                tokio::fs::copy(&src, output).await?;
                return Ok(());
            }
            let args = vec![
                "-y".into(),
                "-i".into(),
                src.to_string_lossy().to_string(),
                "-c".into(),
                "copy".into(),
                output.to_string_lossy().to_string(),
            ];
            ffmpeg.run_ffmpeg(&args).await?;
        }
        ManifestKind::Hls => {
            let playlist = parts_dir.join("playlist.m3u8");
            write_local_hls_playlist(plan, parts_dir, &playlist).await?;
            let args = vec![
                "-y".into(),
                "-copyts".into(),
                "-i".into(),
                playlist.to_string_lossy().to_string(),
                "-c".into(),
                "copy".into(),
                "-bsf:a".into(),
                "aac_adtstoasc".into(),
                "-movflags".into(),
                "+faststart".into(),
                output.to_string_lossy().to_string(),
            ];
            debug!(?args, "hls remux");
            if let Err(e) = ffmpeg.run_ffmpeg(&args).await {
                let args = vec![
                    "-y".into(),
                    "-copyts".into(),
                    "-i".into(),
                    playlist.to_string_lossy().to_string(),
                    "-c".into(),
                    "copy".into(),
                    "-movflags".into(),
                    "+faststart".into(),
                    output.to_string_lossy().to_string(),
                ];
                ffmpeg.run_ffmpeg(&args).await.map_err(|_| e)?;
            }
        }
        ManifestKind::Dash => {
            let concat = parts_dir.join("dash_concat.mp4");
            {
                use tokio::io::AsyncWriteExt;
                let mut out = tokio::fs::File::create(&concat).await?;
                let mut files: Vec<_> = plan.units.clone();
                files.sort_by_key(|u| u.index);
                for u in files {
                    let data = tokio::fs::read(parts_dir.join(&u.file_name)).await?;
                    out.write_all(&data).await?;
                }
                out.flush().await?;
            }
            let args = vec![
                "-y".into(),
                "-copyts".into(),
                "-i".into(),
                concat.to_string_lossy().to_string(),
                "-c".into(),
                "copy".into(),
                "-start_at_zero".into(),
                "-movflags".into(),
                "+faststart".into(),
                output.to_string_lossy().to_string(),
            ];
            ffmpeg.run_ffmpeg(&args).await?;
        }
    }
    Ok(())
}

async fn write_local_hls_playlist(
    plan: &DownloadPlan,
    parts_dir: &Path,
    playlist: &Path,
) -> Result<(), AssembleError> {
    let mut files: Vec<_> = plan.units.clone();
    files.sort_by_key(|u| u.index);
    let target = files
        .iter()
        .filter_map(|u| u.duration)
        .fold(6.0_f64, f64::max)
        .ceil() as i64;

    let mut body = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n"
    );
    for u in files {
        let path = parts_dir.join(&u.file_name);
        if !path.exists() {
            return Err(AssembleError::Message(format!(
                "missing segment {}",
                u.file_name
            )));
        }
        if u.is_init || u.file_name.contains("init") {
            body.push_str(&format!("#EXT-X-MAP:URI=\"{}\"\n", u.file_name));
        } else {
            let dur = u.duration.unwrap_or(6.0);
            body.push_str(&format!("#EXTINF:{dur:.3},\n"));
            body.push_str(&u.file_name);
            body.push('\n');
        }
    }
    body.push_str("#EXT-X-ENDLIST\n");
    tokio::fs::write(playlist, body).await?;
    Ok(())
}


/// Dual-input HLS mux (video playlist + audio/ playlist) — Swift assembleDemuxedHLSTS.
pub async fn assemble_demuxed_hls(
    ffmpeg: &FFmpegPaths,
    video_plan: &DownloadPlan,
    audio_plan: &DownloadPlan,
    parts_dir: &Path,
    output: &Path,
) -> Result<(), AssembleError> {
    if let Some(parent) = output.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let video_pl = parts_dir.join("playlist.m3u8");
    let audio_dir = parts_dir.join("audio");
    tokio::fs::create_dir_all(&audio_dir).await?;
    let audio_pl = audio_dir.join("playlist.m3u8");
    write_local_hls_playlist(video_plan, parts_dir, &video_pl).await?;
    // Rewrite audio unit names relative to audio/ dir
    let audio_rel = DownloadPlan {
        units: audio_plan
            .units
            .iter()
            .map(|u| {
                let mut u = u.clone();
                if let Some(rest) = u.file_name.strip_prefix("audio/") {
                    u.file_name = rest.to_string();
                }
                u
            })
            .collect(),
        kind: audio_plan.kind,
        local_playlist_hint: None,
    };
    write_local_hls_playlist(&audio_rel, &audio_dir, &audio_pl).await?;

    let args = vec![
        "-y".into(),
        "-allowed_extensions".into(),
        "ALL".into(),
        "-protocol_whitelist".into(),
        "file,crypto,data".into(),
        "-copyts".into(),
        "-i".into(),
        video_pl.to_string_lossy().to_string(),
        "-allowed_extensions".into(),
        "ALL".into(),
        "-protocol_whitelist".into(),
        "file,crypto,data".into(),
        "-i".into(),
        audio_pl.to_string_lossy().to_string(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "1:a:0".into(),
        "-c".into(),
        "copy".into(),
        "-bsf:a".into(),
        "aac_adtstoasc".into(),
        "-movflags".into(),
        "+faststart".into(),
        output.to_string_lossy().to_string(),
    ];
    debug!(?args, "demuxed hls remux");
    if let Err(e) = ffmpeg.run_ffmpeg(&args).await {
        let mut args2 = args.clone();
        args2.retain(|a| a != "aac_adtstoasc" && a != "-bsf:a");
        // rebuild without bsf - simpler fallback
        let args2 = vec![
            "-y".into(),
            "-allowed_extensions".into(),
            "ALL".into(),
            "-copyts".into(),
            "-i".into(),
            video_pl.to_string_lossy().to_string(),
            "-i".into(),
            audio_pl.to_string_lossy().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "1:a:0".into(),
            "-c".into(),
            "copy".into(),
            "-movflags".into(),
            "+faststart".into(),
            output.to_string_lossy().to_string(),
        ];
        ffmpeg.run_ffmpeg(&args2).await.map_err(|_| e)?;
    }
    Ok(())
}
