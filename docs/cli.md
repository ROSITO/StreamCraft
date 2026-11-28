# streamcraft — CLI StreamCraft

CLI headless : détecter, télécharger et assembler des flux **clairs** (HLS / DASH / MP4) sans GUI.  
Binaire : **`streamcraft`** (crate Cargo `streamcraft-cli`).  
Sortie machine : **JSON Lines** sur stdout + **exit code**.

> Pas de contournement DRM, captcha, login, ni déchiffrement. Cookies / referer = rejouer une session déjà accordée à l’utilisateur.

```bash
cargo build -p streamcraft-cli --release   # → target/release/streamcraft
cargo run -p streamcraft-cli -- --help
```

---


## MCP (agents Craft)

Serveur MCP stdio via **rmcp** + racines **cap-std** (D021). **Refuse** de démarrer sans les deux racines :

```bash
streamcraft mcp \
  --automation-read-root "$HOME/Downloads/StreamCraft" \
  --automation-write-root "$HOME/Downloads/StreamCraft"
```

Tools : `doctor`, `download`, `fetch`, `season`, `flix_digest`, `flix_pick`, `status`, `list`, `wait`, `cancel`.
Transport : **NDJSON** lines (craft-jsonrpc / rmcp), pas Content-Length.  
Les exports passent par `--automation-write-root` (env `STREAMCRAFT_EXPORT_DIR` pour les sous-invocations).  
Exemple client : `.mcp.json` à la racine du dépôt.

GUI Craft : `streamcraft-app` (egui + wry). Packaging : `./scripts/package-macos-app.sh` (macOS .app), `./scripts/docker-linux-test.sh` (Linux build+test), `./scripts/package-craft-release.sh` (bare bins).  
Release artefacts (v0.3+) : `StreamCraft.app-macos-aarch64.zip`, `streamcraft-macos-aarch64`, `streamcraft-linux-aarch64`, `streamcraft-app-linux-aarch64` (+ `SHA256SUMS.txt`). Linux bins target **glibc ≥ 2.34** (Debian bookworm+).


## Install (utilisateurs)

### Build local

```bash
cargo build -p streamcraft-cli --release
./scripts/fetch-ffmpeg.sh    # sidecars FFmpeg si besoin
./target/release/streamcraft doctor
```

### PATH recommandé

| | |
|--|--|
| Binaire | `~/.local/opt/streamcraft/streamcraft` (ou lien depuis `target/release/`) |
| Lien PATH | `~/.local/bin/streamcraft` |
| Exports défaut | `~/Downloads/StreamCraft/` |
| État (jobs, flix, prefs) | `~/Library/Application Support/StreamCraft/` (macOS) |

```bash
mkdir -p ~/.local/bin ~/.local/opt/streamcraft
cp target/release/streamcraft ~/.local/opt/streamcraft/
ln -sf ~/.local/opt/streamcraft/streamcraft ~/.local/bin/streamcraft
export PATH="$HOME/.local/bin:$PATH"
streamcraft --help
```

### Premier téléchargement

> **`download` ou `fetch` ?** L'adresse d'une page web (ce que montre la barre du
> navigateur) va à **`fetch`**. **`download`** n'accepte qu'une URL de flux
> (`.m3u8` / `.mpd` / `.mp4`) et refuse une page avec
> « URL does not look like a downloadable media manifest/file ».

Page web → trouver le flux et le télécharger (le flux est aussi cherché dans
l'iframe du lecteur vidéo, un niveau) :

```bash
streamcraft fetch 'https://watch.example/episode/1' --auto-download \
  --title 'Example' --out ~/Downloads/StreamCraft
```

Si `fetch` ne trouve rien, le flux est construit par le JavaScript de la page :
ouvrir la page dans l'app (son navigateur lance le lecteur et liste les flux),
puis copier l'URL `.m3u8` vers `download`.

URL de flux déjà connue (`.m3u8` / `.mpd` / `.mp4`) :

```bash
streamcraft download 'https://cdn.example/video/master.m3u8' \
  --title 'Example' \
  --out ~/Downloads/StreamCraft
```

Mode script / agent (`--json`) — succès = dernière ligne utile avec `completed` + champ `path` :

```bash
streamcraft download 'https://cdn.example/video/master.m3u8' --json --title 'Example'
```

Page HTML → seulement lister les flux :

```bash
streamcraft fetch 'https://watch.example/episode/1' --detect-only --json
```

---

## Quand utiliser quoi

| Besoin | Commande |
|--------|----------|
| URL média déjà connue (`.m3u8` / `.mpd` / `.mp4`) | `download <url>` |
| Page web → découvrir le flux | `fetch <pageURL>` |
| Seulement lister les flux d’une page | `fetch … --detect-only` |
| Page → premier flux → export | `fetch … --auto-download` |
| Lister les épisodes d’une saison | `season <showName\|pageURL>` (défaut list-only) |
| Télécharger toute une saison | `season … --auto-download` |
| Nouveautés des hubs favoris (RSS) | `flix digest` / `flix scan` |
| Choisir une entrée du digest | `flix pick 1` ou `flix pick 1,3 --enqueue` |
| Suivre / attendre / annuler un job | `status` / `wait` / `cancel` |
| Déclencher l’app GUI (si ouverte) | `ping` / `open` (`--via-app` : pas encore traité par l’app, la CLI retombe sur `fetch` simple) |

Règle simple : **URL fichier média → `download`**. **URL page HTML → `fetch`**. **Nom de série ou page saison → `season`**. **Hubs / « quoi de neuf ? » → `flix`** (pas `fetch` sur la homepage).

---

## Utilisation agentique

L’agent (assistant, cron, bot messagerie) **orchestre** `streamcraft` : il parse l’intention utilisateur, lance la bonne commande avec `--json`, lit stdout + exit code, puis répond en langage naturel. Il **ne réimplémente pas** RSS, favoris, ni le pipeline download.

### Rôle de l’agent

| Fait l’agent | Fait le CLI |
|--------------|-------------|
| Comprendre la demande NL | Résoudre favoris / RSS / embeds |
| Choisir `flix` / `season` / `fetch` / `download` | Détecter, télécharger, assembler |
| Proposer un digest numéroté, attendre un choix | Persister `seen` / dernier digest |
| Confirmer un chemin fichier à l’utilisateur | Écrire sous `~/Downloads/StreamCraft` |
| Remonter erreurs (exit code + `event=failed`) | Retries réseau, mux FFmpeg |

### Contrat d’invocation

1. Toujours `--json` (sauf debug humain).
2. Parser **stdout** :
   - `download` / `fetch` / `season` → **JSON Lines** (une ligne = un `AgentEvent`)
   - `flix` → **un seul objet JSON**
3. Stderr = warnings (hubs sans RSS, bootstrap auto…) — loguer, ne pas planter si exit 0.
4. Exit code : `0` ok · `2` usage · `3` réseau · `4` assemble · `5` introuvable · `6` annulé · `7` detect timeout.
5. Succès download : dernière ligne utile `event=completed` + champ `path`.

### Routage intention → commande

| Intention utilisateur (exemples) | Action agent |
|----------------------------------|--------------|
| « Qu’est-ce qui est sorti ? » / « nouveautés » / digest | `flix digest --json --mark-seen` → afficher `1…N` ; si `count=0`, silence ou « rien de neuf » |
| Réponse `1` / `1,3` après un digest | `flix pick 1,3 --enqueue --json` (ou `pick` sans `--enqueue` puis confirmer) |
| « Télécharge la saison de *Show* » | `season 'Show' --season N --auto-download --json` (N=1 si non dit) |
| « Liste les épisodes de *Show* S2 » | `season 'Show' --season 2 --json` (list-only) |
| Lien page watch / épisode | `fetch <url> --auto-download --json` (ou `--detect-only` puis `download`) |
| URL `.m3u8` / `.mpd` / `.mp4` | `download <url> --json --referer <page>` |
| « Annule / status » | `cancel` / `status` / `wait` avec `jobId` |

### Boucle type (messagerie / chat)

```
1. User: « quoi de neuf ? »
2. Agent: flix digest --json --mark-seen
3. Agent → User: liste numérotée (display + source)  OU  « rien de neuf »
4. User: « 1 » ou « 1 et 3 »
5. Agent: flix pick 1,3 --enqueue --json
6. Agent suit les JSON Lines (episode → queued → … → completed / season_done)
7. Agent → User: chemins des fichiers ou message d’échec
```

Saison directe (sans digest) :

```
1. User: « télécharge Example Show saison 1 »
2. Agent: season 'Example Show' --season 1 --auto-download --json
3. Agent → User: résumé (N épisodes) + dossier d’export
```

### Règles dures pour l’agent

- **Ne pas demander l’URL** si un nom de série suffit : `season` résout via favoris / recherche sur les hubs bookmarqués.
- **Ne pas** mettre la phrase NL dans `--title` (`--title 'Telecharge la saison de…'` casse le matching). Extraire `show` + `season`.
- **Ne pas** `fetch` la homepage d’un hub pour « lister le catalogue » → timeout (exit 7). Utiliser `flix` ou `season <nom>`.
- **Ne pas** auto-télécharger tout un digest sans choix utilisateur (sauf consigne explicite).
- Premier run Flix : `flix bootstrap` (ou laisser le bootstrap auto du premier `digest` → `count: 0`).
- Cookies / referer : seulement si l’utilisateur a déjà une session ; pas de bypass login / DRM / captcha.

### Favoris = sites, pas titres

Les favoris StreamCraft sont souvent des **hubs** (homepages). L’agent parle en **noms de séries** ; le CLI mappe vers les pages.

```bash
# Bien
streamcraft season 'Example Show' --season 1 --json --auto-download

# Mal
streamcraft season … --title 'Telecharge la saison de Example Show'
```

### Cron / agent proactif

Fréquence typique : 1–2× / semaine.

```bash
streamcraft flix digest --json --mark-seen
# count > 0 → notifier l’utilisateur (WA, mail, …) avec la liste 1…N
# count = 0 → ne rien envoyer
```

Ensuite l’utilisateur répond `1` / `2,4` → `flix pick … --enqueue --json`.

---

## Commandes

```
streamcraft download <url> [options]
streamcraft fetch <pageURL> [--detect-only|--auto-download] [--title T] [--out DIR] [options]
streamcraft season <pageURL|showName> [--list-only|--auto-download] [--show NAME] [--season N] [options]
streamcraft flix scan|digest|bootstrap|seen|pick … [options]
streamcraft status [jobId]
streamcraft list
streamcraft wait <jobId>
streamcraft cancel <jobId>
streamcraft ping | open <url> | inbox | help
```

### Flix — digest nouveautés (hubs favoris)

Scanne le RSS des **homepages** en favoris (ex. `{origin}/engine/rss.php` sur CMS courants). **Propose** un digest numéroté — **n’auto-télécharge pas**.

```
streamcraft flix scan [--json] [--max N] [--since ISO|Nd]
streamcraft flix digest [--json] [--max N] [--mark-seen]
streamcraft flix bootstrap [--json]
streamcraft flix seen list|clear [--json]
streamcraft flix pick <n|n,m> [--enqueue] [--json]
```

| Commande | Effet |
|----------|--------|
| `scan` | Liste brute RSS (aucun side-effect `seen`) |
| `digest` | Top N non-vus (défaut **8**) ; écrit `flix_last_digest.json` ; 1er run (`seen` vide) = **bootstrap auto** → `count: 0` |
| `bootstrap` | Marque le head RSS actuel comme vu (évite le dump initial) |
| `seen list\|clear` | Inspecte / remet à zéro `flix_seen.json` |
| `pick 1,3` | Résout depuis le **dernier digest** → `show`/`season` ou `url` |
| `pick … --enqueue` | Sous-processus `season … --auto-download` ou `fetch … --auto-download` |

**Favoris :** homepages uniquement (pages catalogue ciblées ignorées comme hub, host conservé).  
**Liens RSS :** host réécrit vers celui du favori bookmarqué (miroir → hub).  
**Hubs sans RSS** → warning **stderr**, skip ; exit 0 si au moins un hub OK.  
**État :** `~/Library/Application Support/StreamCraft/flix_seen.json` + `flix_last_digest.json`.  
**Piège :** ne pas `fetch` la homepage d’un hub pour lister un catalogue → timeout média (exit 7).

#### JSON Flix (`--json`) — un objet stdout (pas JSON Lines)

`digest` :

```json
{
  "updated": "2026-10-17T14:54:00Z",
  "count": 2,
  "items": {
    "1": {
      "n": 1,
      "id": "https://hub.example/show/example-s4",
      "url": "https://hub.example/show/example-s4",
      "title": "Example Show - Saison 4",
      "show": "Example Show",
      "season": 4,
      "mode": "season",
      "display": "Example Show — S4",
      "source": "Hub",
      "pub_date": "Sat, 17 Oct 2026 03:17:57 +0200"
    }
  }
}
```

`scan` : `{ "count", "hubs_ok", "hubs_skipped", "items": [ … ] }`  
`pick` (sans `--enqueue`) : tableau d’items.  
`mode` : `season` si titre `… - Saison N`, sinon `fetch`.

Texte humain (sans `--json`) si `count > 0` :

```text
Flix — 2 nouveautés
1. Example Show — S4 (Hub)
2. Other Show — S2 (Hub)
Réponds 1 / 1,3 ou season « Show » --season N
```

Si `count: 0` : stdout vide (silence), exit 0.

### Options communes

| Flag | Défaut | Rôle |
|------|--------|------|
| `--json` | off | **Obligatoire pour un agent** — JSON Lines (`download`/`fetch`/`season`) ou objet JSON (`flix`) |
| `--cookie <header>` | — | Cookie HTTP brut (`a=1; b=2`) |
| `--referer <url>` | — | Header Referer (souvent la page watch) |
| `--user-agent <ua>` | UA StreamCraft | User-Agent |
| `--out <dir>` | `~/Downloads/StreamCraft` | Dossier d’export final |
| `--kind hls\|dash\|mp4` | inféré | Forcer le type |
| `--title <name>` | auto | Titre export / events (prioritaire) |
| `--show <name>` | auto | Nom de série (`season`) |
| `--season <n>` | depuis l’URL | Numéro de saison (`season`) |
| `--list-only` | défaut `season` | Lister sans télécharger |
| `--timeout <sec>` | `45` | Timeout détection `fetch` / page |
| `--quality best\|ask` | `best` | Préférence HLS |
| `--shared-db` | off | DB partagée avec l’app GUI |
| `--via-app` | off | Écrit l’inbox GUI ; pas encore traité par l’app GUI (repli sur le scraping simple) |
| `--max <n>` | `8` (`digest`) | Plafond items (`flix scan` / `digest`) |
| `--since <ISO\|Nd>` | — | Filtre `scan` (ex. `7d`, `2026-10-01T00:00:00Z`) |
| `--mark-seen` | off | `flix digest` : marque les items proposés comme vus |
| `--enqueue` | off | `flix pick` : lance season/fetch auto-download |

---

## Contrat JSON Lines (stdout)

Toujours lancer avec `--json`. **Une ligne = un événement**, clés triées, flush immédiat.  
Parser ligne par ligne ; ignorer les lignes non-JSON (ne devrait pas arriver en `--json`).

### Schéma (`AgentEvent`)

| Champ | Type | Présent quand |
|-------|------|----------------|
| `event` | string | toujours |
| `jobId` | string (UUID) | jobs download |
| `title` | string | souvent |
| `kind` | `hls` \| `dash` \| `progressive` | détection / jobs |
| `progress` | number 0…1 | progress / assembling / completed |
| `completedUnits` | int | segments / épisodes faits |
| `totalUnits` | int | total connu |
| `path` | string | `completed` — fichier export absolu |
| `url` | string | `detected` / `episode` — URL |
| `message` | string | libellé court / erreur |
| `show` | string | `episode` / `season_done` |
| `season` | int | `episode` / `season_done` |
| `episode` | int | `episode` |

### Valeurs de `event`

| `event` | Signification | Action agent typique |
|---------|---------------|----------------------|
| `detected` | Flux trouvé (`fetch`) | Lire `url` + `title` ; décider download |
| `episode` | Page épisode listée (`season`) | Collecter `url` ; download si besoin |
| `season_done` | Fin d’énumération / saison | Lire `completedUnits` / `totalUnits` |
| `queued` | Job enfilé | Garder `jobId` |
| `progress` | Avancement | Afficher / logger ; ne pas timeout trop court |
| `assembling` | FFmpeg en cours | Attendre `completed` / `failed` |
| `completed` | Succès | Lire `path` ; exit attendu `0` |
| `failed` | Échec | Lire `message` ; voir exit code |
| `cancelled` | Annulé | exit `6` |
| `info` | Message hors job | Log |

### Exemples de lignes

```json
{"event":"detected","kind":"hls","message":"detected","title":"Masterplan (2026)","url":"https://cdn.example/a.m3u8"}
{"episode":4,"event":"episode","message":"episode","season":1,"show":"Example Show","title":"Example Show S01E04","url":"https://watch.example/tv/1668/s/1/e/4"}
{"completedUnits":24,"event":"season_done","message":"listed 24 episodes","season":1,"show":"Example Show","totalUnits":24}
{"event":"queued","jobId":"8DC51B08-…","kind":"progressive","message":"queued","title":"Example Show S01E04"}
{"completedUnits":12,"event":"progress","jobId":"…","kind":"hls","message":"Téléchargement 1% (12/1590)","progress":0.0075,"title":"…","totalUnits":1590}
{"event":"assembling","jobId":"…","kind":"hls","message":"assembling","progress":0.99,"title":"…"}
{"event":"completed","jobId":"…","kind":"hls","message":"completed","path":"/tmp/out/Example Show/Saison 01/S01E04.mp4","progress":1,"title":"…"}
{"event":"failed","jobId":"…","message":"HTTP 403"}
```

---

## Exit codes

| Code | Constante | Signification | Retry ? |
|------|-----------|---------------|---------|
| 0 | success | OK | — |
| 2 | usage | Args invalides | Non — corriger la commande |
| 3 | network | Réseau / générique | Oui (backoff) si transitoire |
| 4 | assemble | Échec FFmpeg | Rare — vérifier Helpers/ffmpeg |
| 5 | notFound | `jobId` inconnu | Non |
| 6 | cancelled | Annulé | — |
| 7 | detectTimeout | `fetch` : aucun média avant timeout | Oui — ↑ `--timeout`, vérifier URL page / lecture auto |

`download` / `fetch --auto-download` sont **synchrones** : le process ne se termine qu’à l’état terminal (completed / failed / cancelled). Pas besoin de `wait` dans ce cas.

---

## Titres

Ordre de priorité :

1. `--title <name>` (gagne toujours sur les events + nom de fichier)
2. Titre page nettoyé (`document.title` / décorations site retirées) via `PageTitleResolver`
3. Fallback path URL **non opaque**
4. Jamais l’ID CDN du `.m3u8` (tokens alphanum longs rejetés)

Pour un agent : passer `--title` dès que le nom contenu est connu (ex. depuis favoris / recherche).

---

## Credentials & pages protégées

- `--referer` : pour `download` d’une URL CDN, mettre l’URL de la page watch.
- `fetch --auto-download` pose automatiquement le referer = `pageURL` si absent.
- `--cookie` : header Cookie brut de la session navigateur si le CDN l’exige.
- Les favoris app / CLI sont dans  
  `~/Library/Application Support/StreamCraft/bookmarks.json`  
  (titre + url) — utile pour trouver une page puis `fetch`.

### Favoris GUI (Craft) ↔ CLI

Même fichier pour l’app egui et le CLI. Dans **streamcraft-app** :

| UI | Effet |
|----|--------|
| ★ / ☆ (barre d’URL) | Toggle de la page courante |
| Sidebar **Favorites** / bouton **Annuaire** | Liste : ouvrir, renommer, supprimer ; **+ Add site** (option hub = origin seulement) |
| Badge **Hub** | Path `/` — source RSS pour `flix digest` et recherche `season '<show>'` |

Sans hubs en favoris : `flix digest` exige `--feed` ; `season` par nom échoue s’il n’y a ni bookmark ni digest flix.

---

## Stockage / où vont les fichiers

| Chemin | Contenu |
|--------|---------|
| `--out <dir>` | **Prioritaire** — dossier d’export final |
| `~/Downloads/StreamCraft` | Défaut si `--out` omis (home de l’user qui lance le CLI) |
| `~/Library/Application Support/StreamCraft/cli-store/` | DB jobs CLI (défaut) |
| `~/Library/Application Support/StreamCraft/downloads.sqlite` | DB app (`--shared-db`) |
| `~/Library/Application Support/StreamCraft/inbox/` | Commandes `--via-app` |
| prefs `parts/` | Segments temporaires avant assemble |

Sous-dossier d’export selon nomenclature :
- plat / titre : `{title}/{title}.mp4`
- série (`season --auto-download`) : `{show}/Saison {season}/S{season}E{episode}.mp4`

Exemple (machine de build, `--out` forcé) :  
`/tmp/streamcraft-out/Example Show/Saison 01/S01E01.mp4`

Ne pas mélanger CLI et GUI sans `--shared-db`.

---

## Workflows agent (recettes)

### A — URL média connue

```bash
"$CLI" download "$MEDIA_URL" \
  --json \
  --kind mp4 \
  --referer "$PAGE_URL" \
  --title "Show S01E04" \
  --out /tmp/streamcraft-out
# exit 0 → dernière ligne event=completed + path
```

### B — Page → détection seule

```bash
"$CLI" fetch "$PAGE_URL" --detect-only --json --timeout 60
# exit 0 → une ou plusieurs lignes event=detected (url, kind, title)
# exit 7 → rien trouvé
```

### C — Page → download auto

```bash
"$CLI" fetch "$PAGE_URL" \
  --auto-download --json \
  --title "Masterplan" \
  --out /tmp/streamcraft-out \
  --timeout 60
# detected → queued → progress… → assembling → completed
```

### D — Saison (liste)

```bash
# Via favoris — pas d’URL
"$CLI" season 'Example Show' --season 1 --json
# → info (resolved) → N× episode → season_done

# URL page saison déjà connue
"$CLI" season 'https://hub.example/show/example-s1' \
  --json --show 'Example Show' --season 1

# Pattern watch-path
"$CLI" season 'https://watch.example/tv/1668/s/1/e/1' \
  --json --show 'Example Show' --season 1
```

Énumération (dans l’ordre) :
1. catalogue JSON/texte type `/data/eps_{id}.txt` (embeds ; ordre lecteurs configuré)
2. pattern URL `…/tv/{id}/s/{S}/e/{E}` (HEAD jusqu’à 2 misses)
3. liens DOM si rien trouvé

Défaut = list-only.  
`fetch` / `season --auto-download` préfèrent une variante HLS **vidéo** (`index-v1…`) à l’audio-only (`index-a1`).

### E — Saison (téléchargement séquentiel)

```bash
"$CLI" season 'Example Show' --season 1 \
  --auto-download --json \
  --out ~/Downloads/StreamCraft \
  --timeout 90
# info → episode → fetch embed → detected → queued → … → season_done
# Fichiers : ~/Downloads/StreamCraft/Example Show/Saison 01/S01E0x.mp4
```

### F — Pseudo-code (résumé)

Voir aussi **[Utilisation agentique](#utilisation-agentique)** (contrat + boucles).

```
if url looks like media (.m3u8|.mpd|.mp4|…):
    run download --json [--referer] [--title] [--cookie]
else if intent is "what's new" / digest / nouveautés:
    run flix digest --json [--mark-seen]
    # user replies 1 / 1,3 → flix pick 1,3 [--enqueue --json]
else if intent is whole season:
    run season SHOW_NAME --json --season N                  # list via favoris
    # or: season SHOW_NAME --auto-download --json --season N
else:
    run fetch --detect-only --json --timeout 60
    if exit == 7: retry with larger timeout or ask user
    pick first (or best) detected.url
    run download detected.url --json --referer page --title (flags.title or detected.title)
parse stdout (JSON Lines for download/fetch/season ; single object for flix)
on completed: return path
on failed: surface message + exit code
```

### G — Flix (digest)

```bash
# Premier déploiement / nouveau Mac : seed without spam
"$CLI" flix bootstrap --json
# → {"marked":10}

# Cron agent (ex. mar/ven) — nouveautés seulement
"$CLI" flix digest --json --mark-seen
# → {"count":0,...}  ou  items 1..N

# L’utilisateur répond « 1 » ou « 1,3 »
"$CLI" flix pick 1 --json
# → [{"mode":"season","show":"Example Show","season":4,"url":"…"}]

"$CLI" flix pick 1 --enqueue --json
# → season 'Example Show' --season 4 --auto-download --json
```

---

## Limites (ne pas demander au CLI)

- DRM (`EXT-X-KEY`, Widevine, FairPlay, CENC…) — hors scope MVP ; ne pas tenter de bypass
- Captcha / Cloudflare interactif
- Choix multi-qualité interactif (`--quality ask` peu utile headless → rester sur `best`)
- iOS / remote control hors machine locale

---

## Fichiers code (référence)

| Fichier | Rôle |
|---------|------|
| `archive/swift/OpenStreamCLI/CLI.swift` | Parsing args, orchestration |
| `archive/swift/OpenStreamCore/AgentJSON.swift` | `AgentEvent` + exit codes |
| `archive/swift/OpenStreamCore/FlixService.swift` | Digest RSS / seen / pick |
| `archive/swift/OpenStreamCore/SeasonResolver.swift` | Nom de série → page via favoris |
| `archive/swift/OpenStreamCore/EpisodeEnumerator.swift` | Catalogue / watch-path / DOM |
| `archive/swift/OpenStreamCore/HeadlessDownloadService.swift` | Queue download sans UI |
| `archive/swift/OpenStreamCore/HeadlessBrowserSession.swift` | WKWebView offscreen + titres / liens épisode |
| `archive/swift/OpenStream/Browser/PageTitleResolver.swift` | Nettoyage titres |

Décisions : **MEMORY.md → D018** (CLI), **D019** (flix).
