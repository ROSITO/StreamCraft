//! Strip fake PNG/JPEG headers in front of MPEG-TS (CDN lure .image).

use std::path::Path;

use tokio::io::AsyncWriteExt;

const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];

/// If file starts with PNG/JPEG then later contains MPEG-TS sync (0x47), strip the lure.
pub async fn normalize_segment_file(path: &Path) -> std::io::Result<()> {
    let data = tokio::fs::read(path).await?;
    if data.len() < 188 {
        return Ok(());
    }
    let has_png = data.starts_with(PNG);
    let has_jpeg = data.starts_with(JPEG);
    if !has_png && !has_jpeg {
        return Ok(());
    }
    // Find first 0x47 aligned-ish TS sync
    if let Some(pos) = data.iter().position(|&b| b == 0x47) {
        if pos > 0 && pos < 32 * 1024 && data.len() - pos >= 188 {
            let cleaned = &data[pos..];
            let mut f = tokio::fs::File::create(path).await?;
            f.write_all(cleaned).await?;
            f.flush().await?;
        }
    }
    Ok(())
}
