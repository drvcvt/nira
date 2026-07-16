# nira

A desktop music player written in Rust, built around the lesson that a
single global state object is the easiest way to ruin a reactive UI.

**Status:** daily-driven. Plays from SoundCloud, Spotify, the hi-res provider
downloads and local files; cross-platform discovery works,
library/likes/scrobbling are wired. Jellyfin is the next big gap.

---

## What nira is

A native, local-first music player for Linux/macOS/Windows that
prioritises a snappy UI even with tens of thousands of tracks indexed.
The streaming providers (SoundCloud, Spotify, the hi-res provider, later Jellyfin) live in
isolated provider crates behind a common trait, so the UI doesn't grow
per-provider branches and a provider going down doesn't take the app
with it.

The headline feature: **cross-platform discovery**. Seed a track on
Spotify, get SoundCloud-playable recommendations (and vice versa), with
the rationale exposed in the UI so it's obvious *why* a track was
picked.

**Non-goals (for now):**

- Web/mobile target. The crate layout doesn't preclude it, but every
  decision optimises for desktop ergonomics first.
- A skinning system or theme editor. One good default theme, period.

---

## What works today

| Area              | State |
|-------------------|-------|
| Audio engine      | rodio for SC progressive streams + librespot 0.8 for Spotify. One canonical volume curve (60 dB log) so backend switches don't jolt the levels. |
| SoundCloud        | Public `client_id` auto-detected from the web player. Search, track resolve, related-tracks feed. No login. |
| Spotify           | OAuth PKCE (user brings their own Developer Client ID). Search, liked songs, artist/album detail. Playback via librespot — **requires Spotify Premium**. |
| the hi-res provider             | Hi-res FLAC search + **download-to-library**: whole albums, single tracks, and any SC/Spotify track via strict the hi-res provider match. Delta downloads — re-running an album only fetches the missing tracks; album pages track how much is already on disk. Tags + cover art are embedded at download time (the CDN streams arrive untagged; the API payload is the metadata source). Token auth: paste your `auth token` from the logged-in the provider web player session (the hi-res provider disabled 3rd-party email/password login). FLAC-first, MP3 only as a last resort; Library → Local badges + filters lossless vs lossy. **Requires a the hi-res provider Studio subscription.** Details: [docs/hires-provider.md](docs/hires-provider.md). |
| Local files       | `provider-local` scans `library_root` (tags via lofty), feeds the Library page's Local tab with lossless/lossy badges + filters. Rescan after every download; no filesystem watcher yet. |
| Discovery         | Cross-provider candidate merge from SoundCloud's `/related` and ListenBrainz's similarity graph, optional Last.fm third source. Dedup by (artist, title), provider badges per row. |
| Queue             | Auto-advance watcher (polls `has_source` falling-edge). Manual next/prev/stop, per-row remove. FLAC-first swap: a queued lossy track with a strict the hi-res provider match plays the FLAC instead. **Gapless** on the rodio side (next track appended to the sink, boundary detected via per-source position). Queue + in-track position persist — a restart resumes where you stopped. |
| Playlists         | Local cross-provider playlists (JSON, likes-tier). Tracks add as rows; whole albums embed as expandable widgets (right-click an album card/banner). Rename, reorder, play/shuffle across both. |
| Loudness          | ReplayGain track gain applied on rodio playback; the hi-res provider downloads get measured (EBU R128) and tagged at download time; librespot normalisation on. |
| Keybinds          | Space play/pause, Ctrl+←/→ prev/next, Ctrl+↑/↓ volume, S/R/L shuffle/repeat/like, V visualizer, Ctrl+/ shortcut sheet. |
| Visualizer        | Fullscreen grayscale spectrum + beat-driven particles (V or the wave button). DSP in Rust (FFT, log bands, beat detection); canvas renders. Rodio sources only — librespot is its own engine. |
| Pages             | Home (For You shelves, Daily Mixes, activity rails), Discover, Search + global search overlay (Ctrl+F / Alt+Space), Library (Saved/Local/Playlists/Spotify tabs), Settings (tabbed), Album detail, Artist detail with tabs. |
| Likes             | Local cross-provider liked-tracks store, persisted as JSON. Anything `Heart`-able lands here regardless of source. |
| Scrobbling        | ListenBrainz outbound, background watcher. No-op until a token + username are set. |
| MPRIS (Linux)     | Play/pause/next/prev/seek + now-playing exposed to the desktop environment. Media keys work. |
| Persistence       | XDG dirs via `directories`. Atomic JSON writes so a kill can't corrupt state. |

---

## What's still missing

- **Filesystem watcher.** The local index rescans on demand (and after
  downloads) but doesn't watch `library_root` for out-of-band changes
  (`notify` is the plan).
- **Jellyfin provider.** Crate-shaped slot exists in the workspace
  layout; no implementation.
- **Polish.** Density modes, Discord rich presence. (The mini-player idea
  became the fullscreen visualizer instead.)

---

## Why the workspace looks like this

The predecessor project emitted a single `catalog-updated` event
carrying a 10k-track `BootstrapState`. Every page subscribed to that
signal and re-rendered when *any* domain changed. The main thread
froze long enough during boot that click events visibly dropped —
buttons rendered, but `onclick` never fired.

nira makes that bug structurally impossible:

- **Crates split by domain, not by layer.** `player/` owns audio.
  `discovery/` owns recommendation. `config/` owns persistence.
  `hooks/` owns reactivity primitives. `components/` owns shared
  widgets. `pages/` owns views. `nira/` is the thin shell that wires
  them.
- **No global state.** Each `hooks::use_*` returns a focused signal
  set scoped to one domain. A mutation in `player` doesn't force
  `library` to diff, and vice versa.
- **Audio engine isolated from UI.** `player/` is a normal Rust crate
  that knows nothing about Dioxus. The UI talks to it through
  commands and reads snapshots — never reaches into its internals.

```
nira/
├── nira/                 shell (window, root component, section dispatch, MPRIS bridge)
│   └── assets/css/       split stylesheets (base, home, player, settings, …) — Dioxus requires assets in the binary crate
├── components/           sidebar, bottombar, global context menu
├── pages/                discover, search + overlay, library, album, artist; home/ and settings/ are module dirs
├── hooks/                per-domain reactivity (use_player, use_library, use_downloads, …) + queue + matching
├── player/               rodio + librespot, history log, transport bus
├── config/               AppConfig load/save under XDG dirs
├── provider-api/         common Provider trait + DTOs
├── provider-spotify/     OAuth PKCE + Web API + librespot wiring
├── provider-soundcloud/  client_id scrape + search + streams
├── provider-hires-provider/       token auth, hi-res FLAC downloads (docs/hires-provider.md)
├── provider-local/       library_root scan, tag read via lofty
├── discovery/            cross-platform candidate merge
└── enrichment/           MusicBrainz / ListenBrainz / Last.fm clients + TTL cache
```

---

## Tech stack

| Layer        | Choice                | Why                                                       |
|--------------|-----------------------|-----------------------------------------------------------|
| UI           | Dioxus 0.7 (desktop)  | Rust-native, fine-grained signals, Wry/Tao window         |
| Audio out    | rodio + cpal          | Cross-platform, low-level enough to control buffering     |
| Spotify      | librespot 0.8         | Spotify Connect-style playback against the OAuth session  |
| Decode       | symphonia (via rodio) | Pure Rust, MP3/FLAC/AAC/OGG/MP4 out of the box            |
| Persistence  | serde_json + XDG dirs | Plain files, no migration story until we need one         |
| HTTP         | reqwest (rustls)      | No system OpenSSL dependency                              |
| Discovery    | MusicBrainz + ListenBrainz + Last.fm | Three open similarity sources, merged           |
| MPRIS        | mpris-server          | Cleanest async D-Bus client for the spec                  |
| Logging      | tracing               | Standard, layered, env-filter friendly                    |

---

## Running it

> **anvil offload:** `anvil cargo -- check/test/build …` works for this
> repo since 2026-07-16 (see `anvil.toml`; the old symlinked-target sync
> bug is fixed). Keep `dx` bundling **local** — the worker has no dx
> install; an ephemeral nix-shell task for that is prepared in
> `anvil.toml` but unproven. Avoid legacy zsh `cargo`→`acargo` wrapper
> functions; invoke `anvil` or `command cargo` directly.

### Hot-reload dev loop

```sh
cargo install dioxus-cli   # one-time, gives you the `dx` command
cd ~/projects/nira
dx serve --platform desktop
```

### Release build

```sh
cd ~/projects/nira
dx build --release --platform desktop --package nira
```

Output lands under `target/dx/nira/release/linux/app/` (macOS/Windows
get their own platform dirs) — a self-contained folder with the `nira`
binary and its `assets/` tree. Run the binary from inside that folder
so the asset path resolves.

### Plain cargo (no dev tooling, no asset bundling)

```sh
command cargo run -p nira --release
```

`command cargo check --workspace` is the fastest "did I break
anything" feedback loop — runs in <1s incremental.

---

## First-run setup

nira works on launch — Home/Discover/Search all use SoundCloud
without any login. To unlock the rest, open **Settings**:

- **Spotify** — register a Developer app at
  [developer.spotify.com](https://developer.spotify.com), set the
  redirect URI to `http://127.0.0.1:7777/callback`, paste the Client
  ID, click *Connect*. A browser tab opens for consent; nira spins up
  a one-shot listener on port 7777 to catch the redirect. Tokens
  persist in `~/.config/nira/spotify-tokens.json` and auto-refresh.
  Premium is required for librespot streaming; metadata and likes work
  on free accounts.
- **ListenBrainz** — paste a token from
  [listenbrainz.org/profile](https://listenbrainz.org/profile) plus
  your LB username. Enables outbound scrobbling and the "Listened
  lately" row on Home.
- **Last.fm** — optional API key for a third discovery signal. Either
  paste it in Settings or set `NIRA_LASTFM_API_KEY` in the environment
  at launch.
- **the hi-res provider** — paste your `auth token` from a logged-in
  the provider web player session to enable hi-res FLAC downloads into your
  library (the hi-res provider Studio subscription required — see
  [docs/hires-provider.md](docs/hires-provider.md)).

All state lives under XDG dirs:

- `~/.config/nira/` — config, OAuth tokens, hand-curated likes.
- `~/.cache/nira/` — discovery cache, scraped SC client_id, play
  history (safe to nuke).

---

## Design ground rules

These keep the codebase from drifting back into the original
mega-state failure mode:

- **Domain crates don't import other domain crates.** `provider-spotify`
  doesn't import `discovery`. They communicate through `hooks::*` or
  message channels.
- **Pages own no state.** They mount `use_*` hooks and pass typed
  props to components. State lives in hooks, not page-locals.
- **No `Rc<RefCell<MegaState>>`.** If a struct contains "almost
  everything," it's wrong. Split it.
- **One CSS system for the shell.** Stylesheets are split by area under
  `nira/assets/css/`, all loaded by the shell; pages contribute classes
  but never ship their own CSS bundles.

---

## Roadmap (what's next)

1. **Library watcher + depth** — `notify`-based re-scan of
   `library_root`, richer albums/artists views over the local index,
   virtual scrolling at scale.
2. **Jellyfin provider** — read-only first, same `Provider` trait as
   SC/SP so the UI doesn't need new branches.
3. **Polish** — density modes, Discord rich presence, visualizer presets.

---

## License

Apache-2.0. See [LICENSE](LICENSE).
