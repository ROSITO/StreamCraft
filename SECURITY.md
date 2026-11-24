# Security Policy

## Supported versions

Security fixes target the latest release on [GitHub Releases](https://github.com/ROSITO/StreamCraft/releases) and the `main` branch.

## Scope

StreamCraft downloads **clear** media streams the user can already access. It does **not**:

- bypass DRM (Widevine, FairPlay, PlayReady, CENC, `EXT-X-KEY`, …)
- decrypt protected content
- circumvent authentication or captchas

Please do **not** open issues asking for those capabilities.

## Reporting a vulnerability

Prefer a **private** [GitHub security advisory](https://github.com/ROSITO/StreamCraft/security/advisories/new) on this repository.

Alternatively contact the maintainer via the address on the [GitHub profile](https://github.com/ROSITO).

Include:

- StreamCraft / commit SHA
- OS version
- Steps to reproduce
- Impact (data leak, local code exec, unexpected network, …)

We aim to acknowledge reports within a few days.

## Out of scope

- Issues that only reproduce on DRM-protected streams
- Social-engineering requests for piracy tooling
- Third-party FFmpeg CVEs (report upstream; we vendor sidecars via fetch scripts)
