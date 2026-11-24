import { FormEvent, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type Resolution = { width: number; height: number };
type MediaVariant = {
  id: string;
  bandwidth?: number | null;
  resolution?: Resolution | null;
  playlist_url: string;
  average_bandwidth?: number | null;
};
type DetectedMedia = {
  id: string;
  source_url: string;
  kind: string;
  suggested_title?: string | null;
  page_url?: string | null;
  variants: MediaVariant[];
  preferred_variant_id?: string | null;
  audio_tracks?: { id: string; language?: string | null; name?: string | null; is_default?: boolean }[];
  subtitle_tracks?: { id: string; language?: string | null; name?: string | null; is_default?: boolean }[];
  is_vod: boolean;
  segment_count?: number | null;
  duration_seconds?: number | null;
  content_length?: number | null;
};
type NamingPreset = "flat" | "jellyfin_movie" | "jellyfin_series" | "custom";
type ExportMetadata = {
  title: string;
  year?: string | null;
  show?: string | null;
  season?: string | null;
  episode?: string | null;
  episode_title?: string | null;
  naming_preset?: NamingPreset | null;
};
type Prefs = {
  export_dir: string;
  max_concurrency: number;
  max_concurrent_jobs: number;
  max_retries: number;
  naming_preset: NamingPreset;
  naming_template?: string | null;
  proxy_mode: "system" | "http" | "socks5";
  proxy_url?: string | null;
  proxy_host: string;
  proxy_port: number;
  proxy_username: string;
  proxy_password: string;
  ask_quality: boolean;
  block_popups: boolean;
  adblock: boolean;
  block_redirects: boolean;
  drm_gate: boolean;
  vpn_indicator_filter: "all" | "ignore_tailscale" | "nord_vpn_only";
  auto_enqueue_hls: boolean;
  auto_enqueue_dash: boolean;
  auto_enqueue_progressive: boolean;
};
type SiteBookmark = {
  id: string;
  title: string;
  url: string;
  created_at?: string;
  last_visited_at?: string | null;
};
type RawCandidate = {
  url: string;
  mime_type?: string | null;
  page_url?: string | null;
  source: string;
  observed_at?: string;
};
type DownloadJob = {
  id: string;
  media: DetectedMedia;
  state: string;
  progress: number;
  destination?: string | null;
  error_message?: string | null;
};
type HistoryEntry = { id: string; title: string; source_url: string; output_path: string; completed_at: string };
type FFmpegStatus = { ok: boolean; source?: string; path?: string };
type VpnStatus = {
  is_active: boolean;
  short_label: string;
  detail: string;
  tailscale_connected: boolean;
  nordvpn_connected: boolean;
};

function isTauri() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
function kindLabel(kind: string) {
  if (kind === "hls") return "HLS";
  if (kind === "dash") return "DASH";
  if (kind === "progressive") return "MP4";
  return kind.toUpperCase();
}
function kindClass(kind: string) {
  if (kind === "hls") return "kind-hls";
  if (kind === "dash") return "kind-dash";
  return "kind-mp4";
}
function stateLabel(s: string) {
  switch (s) {
    case "queued": return "En file";
    case "downloading": return "Téléchargement";
    case "assembling": return "Assemblage";
    case "completed": return "Terminé";
    case "failed": return "Échec";
    case "cancelled": return "Annulé";
    default: return s;
  }
}
function formatDuration(seconds?: number | null): string | null {
  if (seconds == null || !Number.isFinite(seconds) || seconds <= 0) return null;
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m > 0) return `${m}m ${String(s).padStart(2, "0")}s`;
  return `${s}s`;
}
function formatBandwidth(bps?: number | null): string | null {
  if (bps == null || bps <= 0) return null;
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Mbps`;
  if (bps >= 1_000) return `${Math.round(bps / 1_000)} kbps`;
  return `${bps} bps`;
}
function hostOf(url: string): string {
  try { return new URL(url).host || url; } catch { return url; }
}
function filenameOf(url: string): string {
  try {
    const parts = new URL(url).pathname.split("/").filter(Boolean);
    return parts[parts.length - 1] || "playlist";
  } catch { return "playlist"; }
}
function preferredVariant(media: DetectedMedia): MediaVariant | null {
  const variants = media.variants ?? [];
  if (!variants.length) return null;
  if (media.preferred_variant_id) {
    const hit = variants.find((v) => v.id === media.preferred_variant_id);
    if (hit) return hit;
  }
  return [...variants].sort((a, b) => (b.bandwidth ?? 0) - (a.bandwidth ?? 0))[0] ?? null;
}
function variantLabel(v: MediaVariant): string {
  const parts = [filenameOf(v.playlist_url)];
  if (v.resolution && v.resolution.width > 0) parts.push(`${v.resolution.width}×${v.resolution.height}`);
  const bw = formatBandwidth(v.bandwidth ?? v.average_bandwidth);
  if (bw) parts.push(bw);
  return parts.join(" · ");
}
function displayTitle(media: DetectedMedia): string {
  return media.suggested_title?.trim() || filenameOf(media.source_url);
}
function mediaScore(m: DetectedMedia): number {
  return (m.variants?.length ?? 0) + (m.segment_count ?? 0) + (m.duration_seconds ? 10 : 0);
}
function upsertDetected(prev: DetectedMedia[], media: DetectedMedia): DetectedMedia[] {
  const idx = prev.findIndex((m) => m.source_url === media.source_url);
  if (idx < 0) return [media, ...prev];
  if (mediaScore(media) >= mediaScore(prev[idx])) {
    const next = [...prev];
    next[idx] = { ...prev[idx], ...media, id: prev[idx].id, variants: media.variants ?? [] };
    return next;
  }
  return prev;
}
function parseTitleYear(raw: string): { title: string; year: string } {
  const m = raw.match(/^(.*?)[\s\-_]*\((\d{4})\)(.*)$/);
  if (m) return { title: `${m[1]}${m[3]}`.replace(/\s+/g, " ").trim() || raw, year: m[2] };
  const m2 = raw.match(/^(.*?)[\s\-_.]*(\d{4})(.*)$/);
  if (m2 && +m2[2] >= 1900 && +m2[2] <= 2100) {
    return { title: `${m2[1]}${m2[3]}`.replace(/\s+/g, " ").trim() || raw, year: m2[2] };
  }
  return { title: raw.trim() || "video", year: "" };
}
function inferSeasonFromUrl(url?: string | null): string {
  if (!url) return "1";
  const m = url.match(/saison[_-]?(\d+)/i) || url.match(/\/s(?:eason)?[\/_-]?(\d+)/i);
  return m?.[1] ?? "1";
}
function looksLikeSeries(url?: string | null, title?: string): boolean {
  const hay = `${url ?? ""} ${title ?? ""}`.toLowerCase();
  return /saison|season|episode|épisode|\/s\d|\/e\d/.test(hay);
}
function presetTemplate(p: NamingPreset): string {
  switch (p) {
    case "flat": return "{title}.{ext}";
    case "jellyfin_movie": return "{title} ({year})/{title}.{ext}";
    case "jellyfin_series": return "{show}/Saison {season}/S{season}E{episode}.{ext}";
    default: return "{title} ({year})/{title}.{ext}";
  }
}
function presetHint(p: NamingPreset): string {
  switch (p) {
    case "flat": return "Un seul fichier : Titre.mp4 dans le dossier d’export.";
    case "jellyfin_movie": return "Film (2020)/Film.mp4 — dossier = nom + année, fichier = nom.";
    case "jellyfin_series": return "Série/Saison 01/S01E01.mp4 — renseignez série, saison et épisode.";
    default: return "Tokens : {title} {year} {show} {season} {episode} {kind} {ext}";
  }
}

const DEFAULT_URL = "https://fs016.lol/15140968-visionquest-saison-1-2026.html";

export default function App() {
  const [url, setUrl] = useState(DEFAULT_URL);
  const [ffmpeg, setFfmpeg] = useState<FFmpegStatus | null>(null);
  const [detected, setDetected] = useState<DetectedMedia[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [jobs, setJobs] = useState<DownloadJob[]>([]);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [vpn, setVpn] = useState<VpnStatus | null>(null);
  const [status, setStatus] = useState("Lance la lecture — le stream en cours apparaît ici.");
  const [downloadingId, setDownloadingId] = useState<string | null>(null);
  const [sheet, setSheet] = useState<"none" | "settings" | "history" | "bookmarks" | "batch">("none");
  const [downloadMedia, setDownloadMedia] = useState<DetectedMedia | null>(null);
  const [syncTarget, setSyncTarget] = useState<{ path: string; title: string } | null>(null);
  const [cleanupMsg, setCleanupMsg] = useState<string | null>(null);
  const [bookmarks, setBookmarks] = useState<SiteBookmark[]>([]);
  const [bookmarked, setBookmarked] = useState(false);
  const [rawCandidates, setRawCandidates] = useState<RawCandidate[]>([]);
  const [showRaw, setShowRaw] = useState(false);
  const [cliPaths, setCliPaths] = useState<{ inbox: string; via_app_addr: string } | null>(null);
  const autoOfferKeys = useRef<Set<string>>(new Set());
  const browserHostRef = useRef<HTMLDivElement | null>(null);

  const refreshJobs = useCallback(() => {
    if (!isTauri()) return;
    invoke<DownloadJob[]>("list_jobs").then(setJobs).catch(() => {});
  }, []);
  const refreshHistory = useCallback(() => {
    if (!isTauri()) return;
    invoke<HistoryEntry[]>("list_history").then(setHistory).catch(() => {});
  }, []);
  const refreshVpn = useCallback(() => {
    if (!isTauri()) return;
    invoke<VpnStatus>("vpn_status").then(setVpn).catch(() => {});
  }, []);
  const refreshBookmarks = useCallback(() => {
    if (!isTauri()) return;
    invoke<SiteBookmark[]>("list_bookmarks").then((items) => {
      setBookmarks(items);
      setBookmarked(items.some((b) => {
        try {
          const a = new URL(b.url).href.replace(/\/$/, "");
          const c = new URL(url).href.replace(/\/$/, "");
          return a === c || new URL(b.url).origin + "/" === new URL(url).origin + "/";
        } catch { return false; }
      }));
    }).catch(() => {});
  }, [url]);
  const refreshRaw = useCallback(() => {
    if (!isTauri()) return;
    invoke<RawCandidate[]>("list_raw_candidates").then(setRawCandidates).catch(() => {});
  }, []);

  const syncBrowserBounds = useCallback(() => {
    if (!isTauri() || !browserHostRef.current) return;
    const r = browserHostRef.current.getBoundingClientRect();
    invoke("browser_set_bounds", {
      x: r.left, y: r.top, width: Math.max(1, r.width), height: Math.max(1, r.height),
    }).catch(() => {});
  }, []);

  const openDownloadSheet = useCallback((media: DetectedMedia) => {
    setDownloadMedia(media);
  }, []);

  const confirmDownload = useCallback(async (
    mediaId: string,
    metadata: ExportMetadata,
    variantId?: string,
    audioTrackId?: string | null,
    subtitleTrackId?: string | null,
  ) => {
    if (!isTauri()) return;
    setDownloadingId(mediaId);
    setDownloadMedia(null);
    setStatus("Démarrage du téléchargement…");
    try {
      const res = await invoke<{ job_id: string; message: string }>("start_download", {
        mediaId,
        metadata,
        variantId: variantId ?? null,
        audioTrackId: audioTrackId ?? null,
        subtitleTrackId: subtitleTrackId ?? null,
      });
      setStatus(`${res.message} (${res.job_id.slice(0, 8)}…)`);
      refreshJobs();
    } catch (e) {
      setStatus(`Erreur download: ${e}`);
    } finally {
      setDownloadingId(null);
    }
  }, [refreshJobs]);

  useEffect(() => {
    if (!isTauri()) return;
    invoke<FFmpegStatus>("ffmpeg_status").then(setFfmpeg).catch(console.error);
    invoke<Prefs>("get_preferences").then(setPrefs).catch(console.error);
    invoke<DetectedMedia[]>("list_detected").then((items) => {
      setDetected(items.map((m) => ({ ...m, variants: m.variants ?? [] })));
      setSelectedId(items[0]?.id ?? null);
    }).catch(() => {});
    refreshJobs();
    refreshHistory();
    refreshVpn();
    refreshBookmarks();
    invoke<{ inbox: string; via_app_addr: string }>("cli_paths").then(setCliPaths).catch(() => {});
    refreshRaw();
    const unlisten = listen<DetectedMedia>("media-detected", (ev) => {
      const media = { ...ev.payload, variants: ev.payload.variants ?? [] };
      setDetected((prev) => upsertDetected(prev, media));
      setSelectedId((cur) => cur ?? media.id);
      setStatus(`Stream détecté (${kindLabel(media.kind)}) — prêt à télécharger`);
      refreshRaw();
      invoke("cookie_refresh").catch(() => {});
    });
    const unlistenAuto = listen<DetectedMedia>("auto-enqueue-offer", (ev) => {
      const media = { ...ev.payload, variants: ev.payload.variants ?? [] };
      const key = media.source_url;
      if (autoOfferKeys.current.has(key)) return;
      autoOfferKeys.current.add(key);
      setDetected((prev) => upsertDetected(prev, media));
      openDownloadSheet(media);
      setStatus(`Auto — sheet téléchargement (${kindLabel(media.kind)})`);
    });
    return () => {
      unlisten.then((f) => f()).catch(() => {});
      unlistenAuto.then((f) => f()).catch(() => {});
    };
  }, [refreshJobs, refreshHistory, refreshVpn, refreshBookmarks, refreshRaw, openDownloadSheet]);

  useEffect(() => {
    if (!isTauri()) return;
    const t = setInterval(refreshJobs, 400);
    const v = setInterval(refreshVpn, 3000);
    const r = setInterval(refreshRaw, 2000);
    return () => { clearInterval(t); clearInterval(v); clearInterval(r); };
  }, [refreshJobs, refreshVpn, refreshRaw]);

  // Child WKWebView paints above React — hide it while any modal/sheet is open.
  useEffect(() => {
    if (!isTauri()) return;
    const overlayOpen = sheet !== "none" || downloadMedia != null || syncTarget != null;
    invoke("browser_set_visible", { visible: !overlayOpen }).catch(() => {});
    if (!overlayOpen) {
      // Re-sync bounds after show (webview may have been zero-sized while hidden).
      requestAnimationFrame(() => {
        const el = browserHostRef.current;
        if (!el) return;
        const r = el.getBoundingClientRect();
        invoke("browser_set_bounds", {
          x: r.left, y: r.top, width: Math.max(1, r.width), height: Math.max(1, r.height),
        }).catch(() => {});
      });
    }
  }, [sheet, downloadMedia, syncTarget]);

  useLayoutEffect(() => {
    syncBrowserBounds();
    const onResize = () => syncBrowserBounds();
    window.addEventListener("resize", onResize);
    const ro = browserHostRef.current ? new ResizeObserver(() => syncBrowserBounds()) : null;
    if (browserHostRef.current && ro) ro.observe(browserHostRef.current);
    const t = setInterval(syncBrowserBounds, 500);
    return () => { window.removeEventListener("resize", onResize); ro?.disconnect(); clearInterval(t); };
  }, [syncBrowserBounds, sheet, downloadMedia]);

  const onNavigate = async (e: FormEvent) => {
    e.preventDefault();
    let normalized = url.trim();
    if (normalized && !/^https?:\/\//i.test(normalized)) {
      normalized = `https://${normalized}`;
      setUrl(normalized);
    }
    if (!isTauri()) return;
    setStatus("Navigation…");
    try {
      await invoke("browser_navigate", { url: normalized });
      setStatus("Page chargée — lance la lecture pour détecter le stream");
      syncBrowserBounds();
    } catch (err) {
      setStatus(`Navigation: ${err}`);
    }
  };

  const ffmpegLabel = useMemo(() => {
    if (!ffmpeg) return "…";
    return ffmpeg.ok ? `FFmpeg OK (${ffmpeg.source})` : "FFmpeg manquant — ./scripts/fetch-bundled-ffmpeg.sh";
  }, [ffmpeg]);

  return (
    <div className="shell">
      <header className="topbar">
        <div className="brand">StreamCraft</div>
        <form className="urlbar" onSubmit={onNavigate}>
          <button
            type="button"
            className={`vpn-badge chrome ${vpn?.is_active ? "on" : ""}`}
            title={vpn?.detail || "VPN"}
            onClick={() => { refreshVpn(); setSheet("settings"); }}
          >
            <span className={`vpn-dot ${vpn?.is_active ? "on" : ""}`} />
            {vpn?.short_label || "VPN…"}
          </button>
          <button
            type="button"
            className={`star ${bookmarked ? "on" : ""}`}
            title={bookmarked ? "Retirer des favoris" : "Ajouter aux favoris"}
            onClick={() => {
              invoke<boolean>("toggle_bookmark", { title: "", url }).then((on) => {
                setBookmarked(on);
                refreshBookmarks();
                setStatus(on ? "Ajouté aux favoris" : "Retiré des favoris");
              }).catch(console.error);
            }}
          >{bookmarked ? "★" : "☆"}</button>
          <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://" spellCheck={false} />
          <button type="submit">Go</button>
          <button type="button" onClick={() => invoke("browser_rescan").then(() => setStatus("Rescan…")).catch(console.error)}>Rescan</button>
        </form>
        <div className="toolbar">
          <button type="button" onClick={() => { refreshBookmarks(); setSheet("bookmarks"); }}>Annuaire</button>
          <button type="button" onClick={() => setSheet("batch")}>Batch</button>
          <button type="button" className="active">Téléchargements</button>
          <button type="button" onClick={() => { refreshHistory(); setSheet("history"); }}>Historique</button>
          <button type="button" onClick={() => setSheet("settings")}>Réglages</button>
          <button type="button" onClick={() => invoke("reveal_exports").catch(console.error)}>Exports</button>
        </div>
      </header>

      {(status || (ffmpeg && !ffmpeg.ok)) && (
        <div className={`banner ${ffmpeg && !ffmpeg.ok ? "warn" : ""}`}>{status || ffmpegLabel}</div>
      )}

      <div className="workspace">
        <aside className="sidebar">
          <div className="side-head">
            <h2>Médias détectés</h2>
            <span className="count">{detected.length}</span>
            <button type="button" className="ghost" disabled={detected.length === 0 && rawCandidates.length === 0}
              onClick={() => { invoke("clear_detected"); setDetected([]); setRawCandidates([]); setSelectedId(null); }}>
              Effacer
            </button>
          </div>
          <div className="side-divider" />
          {detected.length === 0 ? (
            <div className="empty-state">
              <div className="empty-icon">▣</div>
              <strong>Aucun média classifié</strong>
              <p>Les playlists HLS (.m3u8) et fichiers MP4 deviennent des médias ici. Lance la lecture pour détecter le stream en cours.</p>
            </div>
          ) : (
            <ul className="media-list">
              {detected.map((m) => {
                const pref = preferredVariant({ ...m, variants: m.variants ?? [] });
                const duration = formatDuration(m.duration_seconds);
                return (
                  <li key={m.id} className={selectedId === m.id ? "selected" : ""} onClick={() => setSelectedId(m.id)}>
                    <div className="media-row-top">
                      <span className={`badge ${kindClass(m.kind)}`}>{kindLabel(m.kind)}</span>
                      {(m.kind === "hls" || m.kind === "dash") && (m.variants?.length ?? 0) > 1 && (
                        <span className="meta-chip">{m.variants.length} variantes</span>
                      )}
                      {(m.kind === "hls" || m.kind === "dash") && (
                        <span className="meta-chip">{m.is_vod ? "VOD" : "Live?"}</span>
                      )}
                      {(m.audio_tracks?.length ?? 0) > 0 && (
                        <span className="meta-chip">{m.audio_tracks!.length} audio</span>
                      )}
                      {(m.subtitle_tracks?.length ?? 0) > 0 && (
                        <span className="meta-chip">{m.subtitle_tracks!.length} subs</span>
                      )}
                      <button type="button" className="dl-icon" title="Télécharger" aria-label="Télécharger"
                        disabled={downloadingId === m.id}
                        onClick={(e) => { e.stopPropagation(); openDownloadSheet(m); }}>↓</button>
                    </div>
                    <div className="media-title">{displayTitle(m)}</div>
                    {pref && <div className="media-variant">{variantLabel(pref)}</div>}
                    {(m.kind === "hls" || m.kind === "dash") && m.segment_count != null && m.segment_count > 0 ? (
                      <div className="media-timing">
                        <span>{m.segment_count} segments</span>
                        {duration && <><span className="dot">·</span><span className="mono">{duration}</span></>}
                      </div>
                    ) : duration && <div className="media-timing mono">{duration}</div>}
                    <div className="media-host">{hostOf(m.source_url)}</div>
                  </li>
                );
              })}
            </ul>
          )}
          <div className="raw-wrap">
            <button type="button" className="raw-toggle" onClick={() => setShowRaw((v) => !v)}>
              {showRaw ? "▼" : "▶"} Candidats bruts ({rawCandidates.length})
            </button>
            {showRaw && (
              rawCandidates.length === 0 ? (
                <p className="muted tiny pad">Aucun candidat. Segments .ts / URLs non classifiées apparaissent ici.</p>
              ) : (
                <ul className="raw-list">
                  {rawCandidates.slice(0, 80).map((c, i) => (
                    <li key={`${c.url}-${i}`}>
                      <span className="meta-chip">{(c.source || "?").slice(0, 12)}</span>
                      <button type="button" className="ghost tiny" title="Copier"
                        onClick={() => navigator.clipboard?.writeText(c.url).then(() => setStatus("URL copiée"))}>
                        copy
                      </button>
                      <div className="media-host" title={c.url}>{hostOf(c.url)}</div>
                      <div className="raw-url mono" title={c.url}>{c.url}</div>
                    </li>
                  ))}
                </ul>
              )
            )}
          </div>
        </aside>

        <section className="detail">
          <div className="browser-host" ref={browserHostRef} />
          <div className="downloads">
            <div className="dl-head">
              <h2>Téléchargements</h2>
              <span className="count">{jobs.length}</span>
            </div>
            {jobs.length === 0 ? (
              <p className="muted pad">Aucun téléchargement. Cliquez ↓ sur un média détecté.</p>
            ) : (
              <ul className="job-list">
                {jobs.map((j) => {
                  const pct = Math.max(0, Math.min(1, j.progress || 0));
                  const active = j.state === "queued" || j.state === "downloading" || j.state === "assembling";
                  return (
                    <li key={j.id}>
                      <div className="job-top">
                        <strong className="job-title">{j.media.suggested_title || displayTitle(j.media)}</strong>
                        <span className="job-kind">{kindLabel(j.media.kind)}</span>
                      </div>
                      <div className="progress" role="progressbar" aria-valuenow={Math.round(pct * 100)} aria-valuemin={0} aria-valuemax={100}>
                        <div className={j.state === "completed" ? "done" : ""} style={{ width: `${Math.round(pct * 100)}%` }} />
                      </div>
                      <div className="job-bottom">
                        <span className="job-state">
                          {j.error_message ? `Échec · ${j.error_message}` : stateLabel(j.state)}
                          {active ? ` · ${Math.round(pct * 100)}%` : ""}
                        </span>
                        <div className="job-actions">
                          {active && (
                            <button type="button" className="ghost" disabled={j.state === "assembling"}
                              onClick={() => invoke("cancel_job", { jobId: j.id }).then(refreshJobs)}>Annuler</button>
                          )}
                          {(j.state === "cancelled" || j.state === "failed") && (
                            <>
                              <button type="button" className="ghost"
                                onClick={() => invoke("resume_job", { jobId: j.id }).then(refreshJobs)}>Reprendre</button>
                              <button type="button" className="ghost"
                                onClick={() => invoke("assemble_job", { jobId: j.id }).then(refreshJobs)}>Assembler</button>
                              <button type="button" className="ghost"
                                onClick={() => invoke("remove_job", { jobId: j.id }).then(refreshJobs)}>Supprimer</button>
                            </>
                          )}
                          {j.state === "completed" && (
                            <>
                              <button type="button" className="ghost" disabled={!j.destination}
                                onClick={() => j.destination && invoke("open_path", { path: j.destination })}>MP4</button>
                              <button type="button" className="ghost" disabled={!j.destination}
                                onClick={() => j.destination && setSyncTarget({ path: j.destination, title: j.media.suggested_title || displayTitle(j.media) })}>
                                Sync…
                              </button>
                              <button type="button" className="ghost"
                                onClick={() => invoke("remove_job", { jobId: j.id }).then(refreshJobs)}>Supprimer</button>
                            </>
                          )}
                        </div>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </section>
      </div>

      {downloadMedia && (
        <MediaDownloadSheet
          media={downloadMedia}
          defaultPreset={prefs?.naming_preset === "jellyfin_series" || looksLikeSeries(downloadMedia.page_url || url, displayTitle(downloadMedia))
            ? "jellyfin_series"
            : prefs?.naming_preset === "flat" ? "flat" : "jellyfin_movie"}
          onCancel={() => setDownloadMedia(null)}
          onConfirm={(meta, variantId, audioId, subId) => confirmDownload(downloadMedia.id, meta, variantId, audioId, subId)}
        />
      )}

      {sheet === "settings" && prefs && (
        <SettingsSheet
          prefs={prefs}
          setPrefs={setPrefs}
          vpn={vpn}
          ffmpegLabel={ffmpegLabel}
          cleanupMsg={cleanupMsg}
          cliPaths={cliPaths}
          onRefreshVpn={refreshVpn}
          onClose={() => setSheet("none")}
          onSaved={() => setStatus("Préférences enregistrées")}
          onCleanupMsg={setCleanupMsg}
        />
      )}
      {sheet === "bookmarks" && (
        <BookmarkManagerSheet
          bookmarks={bookmarks}
          currentUrl={url}
          onRefresh={refreshBookmarks}
          onNavigate={(u) => { setUrl(u); invoke("browser_navigate", { url: u }).then(() => setSheet("none")); }}
          onClose={() => setSheet("none")}
        />
      )}
      {sheet === "batch" && (
        <BatchSheet
          onClose={() => setSheet("none")}
          onDone={(msg) => { setStatus(msg); setSheet("none"); refreshJobs(); }}
        />
      )}

      {syncTarget && (
        <AvSyncSheet
          target={syncTarget}
          onClose={() => setSyncTarget(null)}
          onDone={(msg) => { setStatus(msg); setSyncTarget(null); refreshJobs(); }}
        />
      )}

      {sheet === "history" && (
        <div className="modal-backdrop" onClick={() => setSheet("none")}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="side-head">
              <h2>Historique</h2>
              <button type="button" className="ghost" onClick={() => setSheet("none")}>Fermer</button>
            </div>
            {history.length === 0 && <p className="muted">Vide.</p>}
            <ul className="job-list">
              {history.map((h) => (
                <li key={h.id}>
                  <strong>{h.title}</strong>
                  <div className="media-host">{h.output_path}</div>
                  <button type="button" className="ghost" onClick={() => invoke("open_path", { path: h.output_path })}>Ouvrir</button>
                </li>
              ))}
            </ul>
          </div>
        </div>
      )}
    </div>
  );
}

function trackLabel(t: { language?: string | null; name?: string | null; is_default?: boolean }, kind: "audio" | "sub") {
  const parts: string[] = [];
  if (t.name?.trim()) parts.push(t.name.trim());
  if (t.language?.trim()) parts.push(t.language.trim().toUpperCase());
  if (!parts.length) parts.push(kind === "audio" ? "Audio" : "Sous-titres");
  if (t.is_default) parts.push("défaut");
  return parts.join(" · ");
}

function preferredAudioId(media: DetectedMedia): string | "" {
  const tracks = media.audio_tracks ?? [];
  if (!tracks.length) return "";
  const french = ["fr", "fra", "fre", "fr-fr", "fr_fr"];
  const hit = tracks.find((t) => {
    const lang = (t.language || "").toLowerCase().replace("_", "-");
    const name = (t.name || "").toLowerCase();
    return french.includes(lang) || lang.startsWith("fr-") || name.includes("français") || name.includes("francais") || name === "french";
  });
  return (hit || tracks.find((t) => t.is_default) || tracks[0])?.id ?? "";
}

function MediaDownloadSheet({
  media, defaultPreset, onConfirm, onCancel,
}: {
  media: DetectedMedia;
  defaultPreset: NamingPreset;
  onConfirm: (meta: ExportMetadata, variantId?: string, audioId?: string | null, subId?: string | null) => void;
  onCancel: () => void;
}) {
  const seed = displayTitle(media);
  const parsed = parseTitleYear(seed);
  const [preset, setPreset] = useState<NamingPreset>(defaultPreset);
  const [title, setTitle] = useState(parsed.title);
  const [year, setYear] = useState(parsed.year);
  const [show, setShow] = useState(parsed.title);
  const [season, setSeason] = useState(inferSeasonFromUrl(media.page_url));
  const [episode, setEpisode] = useState("1");
  const [variantId, setVariantId] = useState(preferredVariant(media)?.id ?? media.variants?.[0]?.id ?? "");
  const [audioId, setAudioId] = useState<string>(preferredAudioId(media));
  const [includeSub, setIncludeSub] = useState(false);
  const [subId, setSubId] = useState<string>(media.subtitle_tracks?.[0]?.id ?? "");
  const [preview, setPreview] = useState("");
  const isSeries = preset === "jellyfin_series";

  useEffect(() => {
    const metadata: ExportMetadata = {
      title: title.trim() || "video",
      year: year.trim() || null,
      show: show.trim() || null,
      season: season.trim() || null,
      episode: episode.trim() || null,
      naming_preset: preset,
    };
    if (isTauri()) {
      invoke<string>("preview_export_path", { metadata }).then(setPreview).catch(() => {
        setPreview(presetTemplate(preset)
          .replace("{title}", title || "video")
          .replace("{year}", year || "")
          .replace("{show}", show || title || "show")
          .replace("{season}", season.padStart(2, "0"))
          .replace("{episode}", episode.padStart(2, "0"))
          .replace("{ext}", "mp4"));
      });
    }
  }, [preset, title, year, show, season, episode]);

  const canConfirm = title.trim() && (!isSeries || (show.trim() && season.trim() && episode.trim()));

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal download-sheet" onClick={(e) => e.stopPropagation()}>
        <h2>Télécharger</h2>
        <div className="seg">
          {(["jellyfin_movie", "jellyfin_series", "flat"] as NamingPreset[]).map((p) => (
            <button key={p} type="button" className={preset === p ? "active" : ""}
              onClick={() => {
                setPreset(p);
                if (p === "jellyfin_series" && !show.trim()) setShow(title);
              }}>
              {p === "jellyfin_movie" ? "Film" : p === "jellyfin_series" ? "Série" : "Plat"}
            </button>
          ))}
        </div>
        <label>
          {isSeries ? "Titre / épisode" : "Ex. Demon Slayer — Le film"}
          <input value={title} onChange={(e) => setTitle(e.target.value)} autoFocus />
        </label>
        {preset === "jellyfin_movie" && (
          <label className="narrow">Année (optionnel)
            <input value={year} onChange={(e) => setYear(e.target.value)} />
          </label>
        )}
        {isSeries && (
          <>
            <label>Nom de la série
              <input value={show} onChange={(e) => setShow(e.target.value)} />
            </label>
            <div className="row2">
              <label>Saison<input value={season} onChange={(e) => setSeason(e.target.value)} /></label>
              <label>Épisode<input value={episode} onChange={(e) => setEpisode(e.target.value)} /></label>
            </div>
          </>
        )}
        <p className="preview mono">{preview || "…"}</p>
        <p className="muted tiny">{presetHint(preset)}</p>
        {(media.variants?.length ?? 0) > 1 && (
          <label>Qualité
            <select value={variantId} onChange={(e) => setVariantId(e.target.value)}>
              {media.variants.map((v) => (
                <option key={v.id} value={v.id}>{variantLabel(v)}</option>
              ))}
            </select>
          </label>
        )}
        {(media.audio_tracks?.length ?? 0) > 0 && (
          <label>Audio
            <select value={audioId} onChange={(e) => setAudioId(e.target.value)}>
              {media.kind !== "dash" && <option value="">Piste intégrée</option>}
              {media.audio_tracks!.map((a) => (
                <option key={a.id} value={a.id}>{trackLabel(a, "audio")}</option>
              ))}
            </select>
          </label>
        )}
        {(media.subtitle_tracks?.length ?? 0) > 0 && (
          <>
            <label className="check">
              <input type="checkbox" checked={includeSub} onChange={(e) => setIncludeSub(e.target.checked)} />
              Inclure les sous-titres
            </label>
            {includeSub && (
              <label>Piste
                <select value={subId} onChange={(e) => setSubId(e.target.value)}>
                  {media.subtitle_tracks!.map((s) => (
                    <option key={s.id} value={s.id}>{trackLabel(s, "sub")}</option>
                  ))}
                </select>
              </label>
            )}
          </>
        )}
        <div className="sheet-actions">
          <button type="button" className="ghost" onClick={onCancel}>Annuler</button>
          <button type="button" className="primary" disabled={!canConfirm}
            onClick={() => onConfirm({
              title: title.trim(),
              year: year.trim() || null,
              show: show.trim() || null,
              season: season.trim() || null,
              episode: episode.trim() || null,
              naming_preset: preset,
            }, variantId || undefined, audioId || null, includeSub ? (subId || null) : null)}>
            Télécharger
          </button>
        </div>
      </div>
    </div>
  );
}

function SettingsSheet({
  prefs, setPrefs, vpn, ffmpegLabel, cleanupMsg, cliPaths, onRefreshVpn, onClose, onSaved, onCleanupMsg,
}: {
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  vpn: VpnStatus | null;
  ffmpegLabel: string;
  cleanupMsg: string | null;
  cliPaths: { inbox: string; via_app_addr: string } | null;
  onRefreshVpn: () => void;
  onClose: () => void;
  onSaved: () => void;
  onCleanupMsg: (s: string | null) => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="side-head">
          <h2>Paramètres</h2>
          <button type="button" className="ghost" onClick={onClose}>Fermer</button>
        </div>

        <section className="settings-sec">
          <h3>Enregistrement</h3>
          <p className="muted">Dossier des MP4</p>
          <p className="mono path">{prefs.export_dir}</p>
          <div className="row2">
            <button type="button" onClick={() => invoke<string>("pick_export_dir").then((d) => setPrefs({ ...prefs, export_dir: d })).catch(() => {})}>Choisir…</button>
            <button type="button" onClick={() => invoke<string>("reset_export_dir").then((d) => setPrefs({ ...prefs, export_dir: d })).catch(console.error)}>Par défaut</button>
          </div>
          <label>Téléchargements simultanés
            <input type="number" min={1} max={4} value={prefs.max_concurrent_jobs}
              onChange={(e) => setPrefs({ ...prefs, max_concurrent_jobs: Number(e.target.value) })} />
          </label>
          <label>Segments en parallèle
            <input type="number" min={1} max={32} value={prefs.max_concurrency}
              onChange={(e) => setPrefs({ ...prefs, max_concurrency: Number(e.target.value) })} />
          </label>
          <p className="muted tiny">16 défaut ; 24–32 sur fibre si le CDN suit.</p>
          <label>Tentatives réseau (retry)
            <input type="number" min={0} max={8} value={prefs.max_retries}
              onChange={(e) => setPrefs({ ...prefs, max_retries: Number(e.target.value) })} />
          </label>
        </section>

        <section className="settings-sec">
          <h3>Nomenclature</h3>
          <label>Modèle par défaut
            <select value={prefs.naming_preset}
              onChange={(e) => setPrefs({ ...prefs, naming_preset: e.target.value as NamingPreset })}>
              <option value="jellyfin_movie">Film</option>
              <option value="jellyfin_series">Série</option>
              <option value="flat">Fichier plat</option>
              <option value="custom">Personnalisé</option>
            </select>
          </label>
          <p className="muted tiny">{presetHint(prefs.naming_preset)}</p>
          {prefs.naming_preset === "custom" && (
            <label>Modèle personnalisé
              <input className="mono" value={prefs.naming_template || ""}
                onChange={(e) => setPrefs({ ...prefs, naming_template: e.target.value })} />
            </label>
          )}
        </section>

        <section className="settings-sec">
          <h3>Qualité HLS</h3>
          <label className="check">
            <input type="radio" checked={!prefs.ask_quality} onChange={() => setPrefs({ ...prefs, ask_quality: false })} />
            Meilleure qualité automatiquement
          </label>
          <label className="check">
            <input type="radio" checked={prefs.ask_quality} onChange={() => setPrefs({ ...prefs, ask_quality: true })} />
            Choisir à chaque téléchargement
          </label>
        </section>

        <section className="settings-sec">
          <h3>Navigateur</h3>
          <label className="check"><input type="checkbox" checked={prefs.block_popups} onChange={(e) => setPrefs({ ...prefs, block_popups: e.target.checked })} />Bloquer les popups</label>
          <label className="check"><input type="checkbox" checked={prefs.adblock} onChange={(e) => setPrefs({ ...prefs, adblock: e.target.checked })} />Bloquer les publicités</label>
          <label className="check"><input type="checkbox" checked={prefs.block_redirects} onChange={(e) => setPrefs({ ...prefs, block_redirects: e.target.checked })} />Bloquer les redirections auto (cross-site)</label>
        </section>

        <section className="settings-sec">
          <h3>Réseau / VPN / Proxy</h3>
          <div className="vpn-row">
            <div>
              <strong className={vpn?.is_active ? "ok" : ""}>{vpn?.short_label || "VPN…"}</strong>
              <p className="muted tiny">{vpn?.detail}</p>
            </div>
            <button type="button" className="ghost" onClick={onRefreshVpn}>Actualiser</button>
          </div>
          <label>Voyant VPN
            <select value={prefs.vpn_indicator_filter}
              onChange={(e) => {
                const vpn_indicator_filter = e.target.value as Prefs["vpn_indicator_filter"];
                setPrefs({ ...prefs, vpn_indicator_filter });
                invoke("save_preferences", { prefs: { ...prefs, vpn_indicator_filter } }).then(onRefreshVpn).catch(() => {});
              }}>
              <option value="all">Tous les VPN connectés</option>
              <option value="ignore_tailscale">Ignorer Tailscale (recommandé)</option>
              <option value="nord_vpn_only">NordVPN uniquement</option>
            </select>
          </label>
          <p className="muted tiny">Le voyant s’appuie sur les VPN Connected dans macOS (scutil), pas sur les apps en arrière-plan.</p>
          <label>Mode téléchargements
            <select value={prefs.proxy_mode}
              onChange={(e) => setPrefs({ ...prefs, proxy_mode: e.target.value as Prefs["proxy_mode"] })}>
              <option value="system">Système (VPN / proxy OS)</option>
              <option value="http">Proxy HTTP</option>
              <option value="socks5">Proxy SOCKS5</option>
            </select>
          </label>
          {prefs.proxy_mode !== "system" && (
            <>
              <label>Hôte<input value={prefs.proxy_host} onChange={(e) => setPrefs({ ...prefs, proxy_host: e.target.value })} /></label>
              <label>Port<input type="number" value={prefs.proxy_port} onChange={(e) => setPrefs({ ...prefs, proxy_port: Number(e.target.value) })} /></label>
              <label>Utilisateur (optionnel)<input value={prefs.proxy_username} onChange={(e) => setPrefs({ ...prefs, proxy_username: e.target.value })} /></label>
              <label>Mot de passe (optionnel)<input type="password" value={prefs.proxy_password} onChange={(e) => setPrefs({ ...prefs, proxy_password: e.target.value })} /></label>
            </>
          )}
        </section>

        <section className="settings-sec">
          <h3>FFmpeg</h3>
          <p className="muted">{ffmpegLabel}</p>
          <label className="check"><input type="checkbox" checked={prefs.drm_gate} onChange={(e) => setPrefs({ ...prefs, drm_gate: e.target.checked })} />Refuser DRM</label>
        </section>

        <section className="settings-sec">
          <h3>Automatisation</h3>
          <p className="muted tiny">Ouvre la feuille de téléchargement à la détection (une fois par URL).</p>
          <label className="check"><input type="checkbox" checked={!!prefs.auto_enqueue_hls} onChange={(e) => setPrefs({ ...prefs, auto_enqueue_hls: e.target.checked })} />Auto HLS</label>
          <label className="check"><input type="checkbox" checked={!!prefs.auto_enqueue_dash} onChange={(e) => setPrefs({ ...prefs, auto_enqueue_dash: e.target.checked })} />Auto DASH</label>
          <label className="check"><input type="checkbox" checked={!!prefs.auto_enqueue_progressive} onChange={(e) => setPrefs({ ...prefs, auto_enqueue_progressive: e.target.checked })} />Auto MP4</label>
        </section>

        <section className="settings-sec">
          <h3>CLI locale</h3>
          <p className="muted tiny">Inbox fichiers + pont TCP via-app</p>
          <p className="mono path">{cliPaths?.inbox || "…"}</p>
          <p className="mono path">{cliPaths?.via_app_addr || "127.0.0.1:8787"}</p>
          <p className="muted tiny">Exemple : openstream-cli status</p>
        </section>

        <section className="settings-sec">
          <h3>Entretien</h3>
          <button type="button" onClick={() => {
            invoke<number>("cleanup_orphan_parts").then((n) => {
              onCleanupMsg(n === 0 ? "Rien à nettoyer." : `${n} élément(s) temporaire(s) supprimé(s).`);
            }).catch(console.error);
          }}>Nettoyer fichiers temporaires</button>
          {cleanupMsg && <p className="muted tiny">{cleanupMsg}</p>}
        </section>

        <button type="button" className="primary" onClick={() => {
          invoke("save_preferences", { prefs }).then(() => { onSaved(); onClose(); }).catch(console.error);
        }}>Enregistrer</button>
      </div>
    </div>
  );
}


function AvSyncSheet({
  target, onClose, onDone,
}: {
  target: { path: string; title: string };
  onClose: () => void;
  onDone: (msg: string) => void;
}) {
  const [timing, setTiming] = useState<{
    video_duration?: number | null;
    audio_duration?: number | null;
    video_start?: number | null;
    audio_start?: number | null;
  } | null>(null);
  const [lead, setLead] = useState(1.0);
  const [shift, setShift] = useState(0);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  const refresh = useCallback(() => {
    invoke<typeof timing>("av_sync_probe", { path: target.path })
      .then((t) => {
        setTiming(t);
        if (t?.audio_duration != null && t?.video_duration != null) {
          const delta = t.audio_duration - t.video_duration;
          if (Math.abs(delta) >= 0.05 && Math.abs(delta) <= 5) setLead(-delta);
        }
      })
      .catch(() => setTiming(null));
  }, [target.path]);

  useEffect(() => { refresh(); }, [refresh]);

  const delta = timing?.audio_duration != null && timing?.video_duration != null
    ? timing.audio_duration - timing.video_duration
    : null;
  const warning = delta != null && Math.abs(delta) > 0.04
    ? (Math.abs(delta) > 1
      ? `Écart A/V ${Math.abs(delta).toFixed(1)}s — segments incomplets possibles.`
      : `Écart A/V ${Math.round(Math.abs(delta) * 1000)} ms — « Recaler » rogne la piste longue.`)
    : null;

  async function run(fn: () => Promise<{ user_message?: string } | string>) {
    setBusy(true);
    setMsg("…");
    try {
      const r = await fn();
      const text = typeof r === "string" ? r : (r as { user_message?: string }).user_message || JSON.stringify(r);
      // AvSyncResult is tagged enum from Rust — format message client-side
      const pretty = formatSyncResult(r);
      setMsg(pretty);
      refresh();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal download-sheet" onClick={(e) => e.stopPropagation()}>
        <h2>Sync A/V</h2>
        <p><strong>{target.title}</strong></p>
        <p className="mono muted tiny">{target.path}</p>
        {timing && (
          <p className="muted tiny">
            V {timing.video_duration?.toFixed(2) ?? "?"}s · A {timing.audio_duration?.toFixed(2) ?? "?"}s
            {delta != null ? ` · Δ ${(delta * 1000).toFixed(0)} ms` : ""}
          </p>
        )}
        {warning && <p className="warn-text">{warning}</p>}

        <section className="settings-sec">
          <h3>Recommandé</h3>
          <p className="muted tiny">Si le son part juste puis dérive, ou si la piste audio est plus courte : recale comme le lecteur (rogne la piste longue).</p>
          <button type="button" className="primary" disabled={busy}
            onClick={() => run(() => invoke("av_sync_realign", { path: target.path }))}>
            {busy ? "…" : "Recaler (skew / piste courte)"}
          </button>
        </section>

        <section className="settings-sec">
          <h3>Dérive progressive</h3>
          <label>Avance audio en fin (s)
            <input type="number" step={0.1} value={lead} onChange={(e) => setLead(Number(e.target.value))} />
          </label>
          <button type="button" disabled={busy}
            onClick={() => run(() => invoke("av_sync_drift", { path: target.path, endAudioLeadSeconds: lead }))}>
            Corriger dérive (atempo)
          </button>
        </section>

        <section className="settings-sec">
          <h3>Décalage constant</h3>
          <label>Décalage audio (s, + = retarder)
            <input type="number" step={0.05} value={shift} onChange={(e) => setShift(Number(e.target.value))} />
          </label>
          <button type="button" disabled={busy}
            onClick={() => run(() => invoke("av_sync_shift", { path: target.path, audioShiftSeconds: shift }))}>
            Appliquer décalage
          </button>
        </section>

        {msg && <p className="muted">{msg}</p>}
        <div className="sheet-actions">
          <button type="button" className="ghost" onClick={() => { if (msg) onDone(msg); else onClose(); }}>Fermer</button>
        </div>
      </div>
    </div>
  );
}



function BookmarkManagerSheet({
  bookmarks, currentUrl, onRefresh, onNavigate, onClose,
}: {
  bookmarks: SiteBookmark[];
  currentUrl: string;
  onRefresh: () => void;
  onNavigate: (url: string) => void;
  onClose: () => void;
}) {
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState("");
  const [url, setUrl] = useState(currentUrl || "https://");
  const [asHub, setAsHub] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  function submitAdd() {
    setError(null);
    invoke("add_bookmark", { title, url, asHub })
      .then(() => { onRefresh(); setAdding(false); })
      .catch((e) => setError(String(e)));
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="side-head">
          <h2>{adding ? "Ajouter un site" : "Annuaire"}</h2>
          <button type="button" className="ghost" onClick={() => adding ? setAdding(false) : onClose()}>
            {adding ? "Annuler" : "Fermer"}
          </button>
          {!adding && (
            <button type="button" onClick={() => { setAdding(true); setTitle(""); setUrl(currentUrl || "https://"); setAsHub(true); }}>+</button>
          )}
        </div>
        {adding ? (
          <>
            <label>Nom affiché<input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="ex. Mon hub" /></label>
            <label>URL<input value={url} onChange={(e) => setUrl(e.target.value)} /></label>
            <label className="check"><input type="checkbox" checked={asHub} onChange={(e) => setAsHub(e.target.checked)} />Enregistrer comme hub (page d’accueil)</label>
            <p className="muted tiny">{asHub ? "Seul le domaine est gardé." : "URL exacte de la page."}</p>
            <button type="button" onClick={() => { setUrl(currentUrl); }}>Utiliser la page courante</button>
            {error && <p className="warn-text">{error}</p>}
            <div className="sheet-actions">
              <button type="button" className="primary" disabled={!url.trim()} onClick={submitAdd}>Ajouter</button>
            </div>
          </>
        ) : bookmarks.length === 0 ? (
          <div className="empty-state">
            <strong>Annuaire vide</strong>
            <p>Ajoutez des hubs ou des pages utiles. Pas de catalogue embarqué.</p>
            <button type="button" className="primary" onClick={() => setAdding(true)}>Ajouter un site…</button>
          </div>
        ) : (
          <ul className="job-list">
            {bookmarks.map((b) => (
              <li key={b.id}>
                <div className="job-top">
                  {editingId === b.id ? (
                    <input value={draft} onChange={(e) => setDraft(e.target.value)}
                      onBlur={() => { invoke("rename_bookmark", { id: b.id, title: draft }).then(() => { setEditingId(null); onRefresh(); }); }} />
                  ) : (
                    <strong>{b.title || hostOf(b.url)}</strong>
                  )}
                  {(() => { try { const u = new URL(b.url); return u.pathname === "/" || u.pathname === ""; } catch { return false; } })() && (
                    <span className="meta-chip">Hub</span>
                  )}
                </div>
                <div className="media-host">{b.url}</div>
                <div className="job-actions">
                  <button type="button" className="ghost" onClick={() => {
                    invoke("open_bookmark", { id: b.id }).then(() => onNavigate(b.url)).catch(() => onNavigate(b.url));
                  }}>Ouvrir</button>
                  <button type="button" className="ghost" onClick={() => { setEditingId(b.id); setDraft(b.title); }}>Renommer</button>
                  <button type="button" className="ghost" onClick={() => invoke("remove_bookmark", { id: b.id }).then(onRefresh)}>Supprimer</button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function BatchSheet({ onClose, onDone }: { onClose: () => void; onDone: (msg: string) => void }) {
  const [text, setText] = useState("");
  const [preview, setPreview] = useState<{ media_count: number; page_count: number } | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!text.trim()) { setPreview(null); return; }
    const t = setTimeout(() => {
      invoke<{ media_count: number; page_count: number }>("preview_batch", { text })
        .then(setPreview)
        .catch(() => setPreview(null));
    }, 200);
    return () => clearTimeout(t);
  }, [text]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="side-head">
          <h2>Batch</h2>
          <button type="button" className="ghost" onClick={onClose}>Fermer</button>
        </div>
        <p className="muted tiny">Collez des URLs (média .m3u8/.mpd/.mp4 ou pages à ouvrir).</p>
        <textarea className="batch-area mono" rows={10} value={text} onChange={(e) => setText(e.target.value)}
          placeholder={"https://cdn.example/stream.m3u8\\nhttps://site.example/watch/1"} />
        {preview && (
          <p className="muted">{preview.media_count} média(s) · {preview.page_count} page(s)</p>
        )}
        <div className="sheet-actions">
          <button type="button" className="ghost" onClick={onClose}>Annuler</button>
          <button type="button" className="primary" disabled={busy || !text.trim()}
            onClick={() => {
              setBusy(true);
              invoke<string>("submit_batch", { text })
                .then((msg) => onDone(msg))
                .catch((e) => { setBusy(false); onDone(String(e)); });
            }}>
            {busy ? "…" : "Lancer"}
          </button>
        </div>
      </div>
    </div>
  );
}


function formatSyncResult(r: unknown): string {
  if (!r || typeof r !== "object") return String(r);
  const o = r as Record<string, unknown>;
  if ("player_aligned" in o) {
    const v = o.player_aligned as { trimmed_seconds: number };
    return `Recalé comme le lecteur (rogne ${Math.round(v.trimmed_seconds * 1000)} ms)`;
  }
  if ("drift_corrected" in o) {
    const v = o.drift_corrected as { duration_delta_seconds: number; atempo: number };
    return `Dérive corrigée (${Math.round(v.duration_delta_seconds * 1000)} ms, atempo ${v.atempo.toFixed(5)})`;
  }
  if ("offset_applied" in o) {
    const v = o.offset_applied as { audio_shift_seconds: number };
    return `Décalage appliqué (${Math.round(v.audio_shift_seconds * 1000)} ms)`;
  }
  if ("already_aligned" in o) {
    const v = o.already_aligned as { delta_seconds: number };
    return `Déjà aligné (Δ ${Math.round(v.delta_seconds * 1000)} ms)`;
  }
  if (o === "missing_audio_or_video" || "MissingAudioOrVideo" in o) return "Fichier sans piste audio et vidéo";
  return JSON.stringify(r);
}

