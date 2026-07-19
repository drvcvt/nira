# Handoff: Review-Backlog 2026-07-18 abgeräumt (drei Wellen)

Date: 2026-07-19
Project: `/home/mt/projects/nira`

## Goal / current status

- Ziel: Die offenen Findings aus dem 4-Agent-Review vom 2026-07-18 (auf
  `fbfd085`) abarbeiten — Reihenfolge laut User: Provider-Reliability,
  dann A11y, danach Queue-Popover + Kleinkram.
- Alles implementiert, kompiliert und Tests grün via `anvil check` /
  `anvil tests`. Drei Commits: `5b95b1e` (Provider), `b32d90c`
  (A11y + Queue-Popover), plus die Misc-Welle.
- Die laufende nira-Instanz war während der Session nicht betroffen; das
  neue Bundle greift beim nächsten Launcher-Start.

## Welle 1 — Provider-Reliability (`5b95b1e`)

the hi-res provider (`provider-hires-provider/src/lib.rs`):
- `getFileUrl` → HTTP 400 heißt Signatur abgelehnt (Bundle-Rotation):
  App-Creds werden verworfen, einmal neu gescraped, Request wiederholt
  (`file_url` / `file_url_once` / `SIG_REJECTED`-Marker). Vorher brickte
  eine Rotation das Streaming bis zum Hand-Löschen von
  `~/.cache/nira/hires-provider-auth.json`.
- 401-Politik in `api_get_raw`: ein 401 wird einmal stillschweigend
  wiederholt; erst der zweite konsekutive 401 loggt aus. Der abgelehnte
  Token wird als `rejected_token` im Auth-Cache tombstoned, und `new()`
  adoptiert einen identischen config.json-Token nicht mehr — das
  Boot-Resurrect-Loop ist damit tot (`adopt_token` + Test).
- `ensure_app_creds` persistiert frisch gescrapte Creds sofort.

Spotify (`provider-spotify/src/lib.rs`, `player/`, `hooks/queue.rs`):
- `SpLikedItem.track` ist jetzt `Option` — ein `track: null`-Ghost-Item
  (aus dem Katalog gezogener Save) killt nicht mehr die ganze
  Liked-Songs-Sync-Seite (`liked_items_to_tracks` + Test). Offset-Mathe
  zählt weiterhin Roh-Items, Ghosts verschieben keine Folgeseiten.
- API-401 trotz nicht abgelaufenem Token → genau EIN erzwungener
  Refresh + Retry (`refresh_after_401`), dedupliziert übers bestehende
  `refresh_lock`: Verlierer des Rennens adoptieren den frischen Token,
  statt den bereits rotierten Refresh-Token zu verbrennen (das hätte
  400 → Logout bedeutet). Refresh-Body in `refresh_with` gefactort.
- Tote librespot-Session (Suspend/Netzwerk): librespot ruft bei
  AP-Transportfehlern selbst `shutdown()` auf → `Session::is_invalid()`.
  `SpotifyBackend::session_is_invalid()` exponiert das;
  `Player::ensure_spotify` erkennt es im Fastpath, dropt und verbindet
  neu. Zusätzlich in `play_one`: schlägt der Connect fehl (AP lehnt
  nicht-abgelaufenen Token nach Suspend ab), einmal
  `refresh_playback_token` + Retry.

Perms (config-Crate):
- `atomic_write_secret_json` / `atomic_write_secret_bg` schreiben mit
  0600 (Modus wird VOR dem Rename auf die Tempdatei gesetzt).
  config.json (`save_bg`) und hires-provider-auth.json nutzen das;
  `tighten_secret_perms` repariert Altbestände beim Boot/Load.

## Welle 2 — A11y + Queue-Popover (`b32d90c`)

- Fokusring: `:focus-visible` jetzt 2px `--sub` (vorher 1px `--faint`,
  <3:1 in beiden Themes). Track-Row-Fokus als Outline — der alte
  inset-box-shadow wurde vom mt-ui-style-Blanket
  `box-shadow:none !important` gefressen. WICHTIG für die Zukunft:
  Fokus-Indikatoren in diesem Repo IMMER als outline, nie box-shadow.
- Datentext (`.track-duration`, `.player-time`, Queue-Indizes/-Dauern,
  `.vol-pct`) von `--faint` auf `--sub`/`--text-muted` (WCAG AA).
- aria-labels auf allen Icon-only-Controls (Transport, Like, Viz,
  Queue, Volume-Slider, Toast-Dismiss, Queue-Row-Remove, corner-search,
  SearchBar-Input); Shuffle mit `aria-pressed`, Queue-Button mit
  `aria-expanded`.
- Kontextmenü: `role="menuitem"` überall, Auto-Fokus aufs erste Item
  beim Öffnen, ArrowUp/Down roving focus (`ctx_focus_move` via
  `document::eval`). Der Shell-JS-Hotkey-Listener returned früh solange
  `.ctx-menu` offen ist — sonst hätten die Pfeile die Volume-Binds
  getroffen.
- Queue-Popover: Escape (Bridge `nira-key-queue-close` im JS-Listener,
  nach dem Search-Overlay-Check) + Click-outside (`.queue-overlay`,
  gleiches Muster wie ctx-overlay). Rows `tabindex=0` + Enter spielt
  (Space bleibt global Play/Pause). Rendering gefenstert: 250 Rows um
  den aktuellen Index, „Show more"-Row verlängert
  (`QUEUE_WINDOW`-ponytail-Kommentar: echte Virtualisierung erst wenn
  nötig).
- Targets: 22px-Buttons (`.track-row-move`, `.download-toast-close`)
  und Queue-Close/-Remove auf 28px.

## Welle 3 — Misc

- `atomic_write`: fsync vor dem Rename (`write_synced`) — Powerloss
  kann keine Null-Byte-Statedateien mehr hinterlassen. Boot-Sweep
  `config::sweep_stale_tmp_files()` räumt verwaiste
  `.<name>.tmp-<pid>-<n>` in config-/cache-Root (eigene PID bleibt) —
  Aufruf in `main()`, Test `sweep_removes_foreign_tmp...`.
- history.jsonl: kein Sync-IO mehr auf dem Playback-Thread — jede
  Mutation serialisiert den Snapshot (≤500 Zeilen) und fährt über die
  persist-FIFO (`atomic_write_bg`), Clear über `remove_bg` (Ordnung!).
  player-Crate hängt jetzt an der config-Crate.
- SC HLS: Segment-Fehler → einmal Retry, dann Fehler statt stiller
  Truncation (Loch verschiebt Timeline, fehlender Tail sah aus wie
  „Track endet zufällig früher"). 403 zählt wie 401 als AuthRequired,
  damit `with_client_id` die rotierte client_id refresht.
- MPRIS: Seeked-Signal nur noch bei has_source && !is_paused (vorher
  D-Bus-Spam alle 500ms auch im Idle).
- Auto-Advance-Wedge: der Watcher-Arm ignoriert jetzt `is_paused`, wenn
  die Source gedraint ist — eine Pause exakt am Trackende gehört zu
  einer Source, die nicht mehr existiert; Load-Pfade rufen
  `rodio.play()`, der Advance entpaust also.
- Shuffle-Restore: `PersistedQueue` trägt `pre_shuffle` (serde
  default) — eine geshuffelte Queue von Disk kann jetzt wieder
  un-shuffled werden.
- rodio bekommt das `tracing`-Feature — cpal „audio stream error" landet
  im Logfile statt nackt auf stderr.
- Tot: `pages/src/search.rs` gelöscht (nie gemountet, Section-Enum hat
  keinen Search-Eintrag; Overlay ist die echte Suche) + die drei
  ungenutzten Butterchurn-Preset-Packs (~1.7MB, Visualizer injiziert
  nur base; PROVENANCE.txt aktualisiert).
- Tokens: `--scrim`, `--on-image-bg/fg` in base.css; search-Backdrop
  und `.provider-badge` nutzen sie (bewusst fix dunkel in beiden
  Themes — liegen auf Artwork).
- Features: Mute-Toggle (M-Taste + Speaker-Button in der Bottombar,
  geteilter `MuteStash`-Context in `components::hotkeys`; merkt sich
  Pre-Mute-Volume, Fallback 50%); Shift+←/→ seekt ±10s (clamp kurz vors
  Ende, damit der natürliche Advance greift); Player-Titel hat Tooltip;
  Toast-Text ist selektierbar (`user-select: text`) für Bug-Reports.
  Shortcut-Sheet um beide ergänzt.

## Nicht gemacht / bewusst offen

- Dead-CSS-Audit der alten Search-Page-Iterationen (Overlay teilt
  Klassen — braucht sorgfältige Prüfung, wenig Wert).
- Echte Queue-Virtualisierung (ponytail-Kommentar markiert den Pfad).
- `curated-presets.json` könnte Presets aus den gelöschten Extra-Packs
  listen — der Runtime-Filter toleriert das (fehlende Presets werden
  einfach nicht gefunden).

## Verification

- `anvil check` + `anvil tests` grün nach jeder Welle (Exit 0).
- UI-Wellen headless nicht sinnvoll verifizierbar (Pointer-Flows, siehe
  Memory zum Test-Rig) — Sichtprüfung beim nächsten App-Start:
  Fokusringe (Tab durch Track-Listen), Rechtsklick-Menü mit Pfeiltasten,
  Esc/Click-outside auf der Queue, M/Shift+←→.
