//! Strip fake PNG/JPEG/GIF headers in front of MPEG-TS or fMP4 (CDN lures),
//! as Swift `MediaSegmentNormalizer` did.

use std::path::Path;

use tokio::io::AsyncWriteExt;

const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];
const GIF87: &[u8] = b"GIF87a";
const GIF89: &[u8] = b"GIF89a";

/// If the file starts with an image header hiding TS or fMP4 data, strip the lure.
pub async fn normalize_segment_file(path: &Path) -> std::io::Result<()> {
    let data = tokio::fs::read(path).await?;
    if data.len() < 188 {
        return Ok(());
    }
    if ![PNG, JPEG, GIF87, GIF89].iter().any(|sig| data.starts_with(sig)) {
        return Ok(());
    }
    if let Some(pos) = ts_sync_offset(&data).or_else(|| fmp4_offset(&data))
        && pos > 0
    {
        let mut f = tokio::fs::File::create(path).await?;
        f.write_all(&data[pos..]).await?;
        f.flush().await?;
    }
    Ok(())
}

/// First offset (within 32 KiB) where 0x47 repeats every 188 bytes — a real TS
/// sync, not a stray 0x47 (the PNG signature itself has one at index 3).
fn ts_sync_offset(data: &[u8]) -> Option<usize> {
    const PKT: usize = 188;
    let need = if data.len() >= 3 * PKT { 3 } else { 2 };
    (0..data.len().min(32 * 1024)).find(|&p| {
        (0..need).all(|k| data.get(p + k * PKT) == Some(&0x47))
    })
}

/// Start of the first ISO-BMFF box (`ftyp`/`styp`/`moof`/`sidx`) within 64 KiB.
fn fmp4_offset(data: &[u8]) -> Option<usize> {
    let limit = data.len().min(64 * 1024);
    (4..limit.saturating_sub(4)).find_map(|i| {
        matches!(&data[i..i + 4], b"ftyp" | b"styp" | b"moof" | b"sidx").then(|| i - 4)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_lure_is_stripped_at_real_ts_sync() {
        let lure_len = 70; // PNG sig + junk, contains 0x47 at index 3
        let mut data = PNG.to_vec();
        data.resize(lure_len, 0x47); // extra stray sync bytes in the lure
        data[lure_len - 1] = 0x00;
        let ts_start = data.len();
        for _ in 0..4 {
            let mut pkt = vec![0u8; 188];
            pkt[0] = 0x47;
            data.extend(pkt);
        }
        assert_eq!(ts_sync_offset(&data), Some(ts_start));
    }

    #[test]
    fn gif_lure_before_fmp4_is_found() {
        let mut data = b"GIF89a\x01\x00\x01\x00junk".to_vec();
        let start = data.len();
        data.extend_from_slice(&[0, 0, 0, 24]);
        data.extend_from_slice(b"ftypisom");
        data.resize(start + 400, 0);
        assert_eq!(ts_sync_offset(&data), None);
        assert_eq!(fmp4_offset(&data), Some(start));
    }
}
