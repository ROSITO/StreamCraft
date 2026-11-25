# CLAUDE.md — StreamCraft

Instructions permanentes pour tout agent (Cursor, Claude Code, Codex) travaillant sur ce dépôt.

---

## Projet

**StreamCraft** (anciennement OpenStream) est une application **desktop cross-platform** (macOS, Windows, Linux) open source qui télécharge des flux vidéo (HLS, MPEG-DASH, MP4 progressif).

- Stack : **Rust 2024**, Tauri 2, React/Vite, Tokio, reqwest, rusqlite, FFmpeg sidecar
- CLI headless : binaire **`streamcraft`** (crate Cargo `openstream-cli`)
- Legacy Swift : `archive/swift/` (gelé, tag `pre-rust`)
- Licence : **MIT** (`LICENSE`)

Documents de référence (lire dans cet ordre) :

1. `PRD_OpenStream.md` — vision et périmètre (nom de fichier historique ; produit = StreamCraft)
2. `ARCHITECTURE.md` — modules, frontières, contrats
3. `ROADMAP.md` — phases et critères de done
4. `MEMORY.md` — décisions figées et pièges connus
5. `docs/cli.md` — contrat agentique (`streamcraft` : routage NL, JSON / `season` / `flix`, exit codes)
6. `README.md` — install / build public

---

## Règles absolues

### Légal / éthique

- **Jamais** de code qui contourne DRM (Widevine, FairPlay, PlayReady), chiffrement, ou authentification. Aucun déchiffrement de flux protégés.
- **Version de test / MVP interne :** on s'affranchit de la *partie DRM produit* (détection + refus + messaging) pour simplifier la mise en œuvre. Cible : flux clairs / fixtures. Pas de gate DRM obligatoire avant la release publique.
- **Déploiement open source public :** réintégrer la gate DRM (voir ROADMAP Phase 9) — détecter `EXT-X-KEY`, `ContentProtection`, CENC, etc., afficher un message clair, **refuser** le téléchargement.
- Les cookies / headers de session ne servent qu'à rejouer l'accès déjà accordé à l'utilisateur dans le navigateur intégré — pas à bypasser un login.

### Architecture

- Respecter les frontières de modules décrites dans `ARCHITECTURE.md`.
- Aucun module UI n'importe directement FFmpeg, les parsers HLS/DASH, ou la couche SQLite.
- Les dépendances vont **uniquement vers le bas** du pipeline (UI → Core → Browser → Network → Detector → Parser → Download → FFmpeg → Exporter).
- Préférer des `protocol` + injection de dépendances aux singletons globaux (sauf `Logger` / `Preferences` si justifié).

### Rust / qualité

- Rust edition 2024, Tokio pour l'async ; erreurs typées (`thiserror` / `anyhow` aux frontières bin).
- Logs via `tracing` (pas de `println!` en prod hors contrat CLI JSON Lines).
- Dépendances workspace dans le `Cargo.toml` racine ; crates : `openstream-core`, `openstream-cli` → **`streamcraft`**, `openstream-ffmpeg`, `apps/desktop`.
- Chaque crate livrée avec tests unitaires ciblés avant la phase ROADMAP suivante.
- FFmpeg **uniquement** via process/sidecar (`openstream-ffmpeg`) — pas de link libav*.

### Git / fichiers

- Ne pas committer de secrets, clés API, binaires FFmpeg précompilés volumineux : utiliser `scripts/fetch-ffmpeg.sh` → `resources/ffmpeg/` (gitignorés).
- Ne pas créer de docs markdown non demandés.
- Commits uniquement sur demande explicite de l'utilisateur.

---

## Workflow agent

1. Lire `MEMORY.md` avant toute décision d'architecture non triviale.
2. Travailler **une phase ROADMAP à la fois** ; ne pas anticiper V2/V3 pendant le MVP sauf si nécessaire pour un contrat stable.
3. Après une décision structurante : mettre à jour `MEMORY.md` (section Décisions).
4. Après livraison d'une phase : cocher les critères de done dans `ROADMAP.md`.
5. En cas de conflit PRD ↔ ARCHITECTURE ↔ code : PRD gagne sur le périmètre ; ARCHITECTURE gagne sur les frontières techniques ; noter le conflit dans `MEMORY.md`.

---

## Hors scope (rappel)

- Contournement DRM / extraction de contenus chiffrés / déchiffrement
- Contournement d'authentification
- Gate DRM produit (détection + refus UI) — **reportée** à la Phase 9 (release publique), pas dans le chemin test/MVP
- Support iOS / iPadOS
- Contournement DRM / CEF embarqué
- Dual-maintenance Swift + Rust
