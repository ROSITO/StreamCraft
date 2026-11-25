# FFmpeg sidecars

StreamCraft invokes `ffmpeg` / `ffprobe` as external processes (LGPL/GPL binaries, not linked).

Fetch:

```bash
./scripts/fetch-ffmpeg.sh
```

Installs into `resources/ffmpeg/{os-arch}/` where `{os-arch}` matches Rust `std::env::consts` (`macos-aarch64`, `linux-x86_64`, `windows-x86_64`, …).

Resolution order (`streamcraft-ffmpeg`): env `STREAMCRAFT_FFMPEG`/`STREAMCRAFT_FFPROBE` → bundled helpers → PATH.
