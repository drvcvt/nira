# nira — Discovery-First Pivot Design

**Date:** 2026-05-16
**Status:** Approved (sections 1–3); section 4 phasing pending refinement during implementation.

## Vision

A Dioxus desktop streaming client with two backends — Spotify (via librespot) and SoundCloud (via web-stream client_id auto-detect) — wrapped around a **cross-platform sonic-similarity discovery engine** as the differentiator. UX target: feels like kopuz (snappy, themable, polished), but the centre of gravity is on-demand discovery, not local-library browsing.

Local files are out of scope for v1 but the crate layout leaves the door open.

## Decisions

| Topic | Choice |
|-------|--------|
| Spotify playback | librespot in-process; Spotify Premium required |
| SoundCloud role | Full audio source + cross-platform bridge target; client_id auto-detect (gray-area, established) |
| Local files | Deferred to later phase |
| Discovery focus | On-demand, algorithmic; USP = sonic-similarity with cross-platform resolution |
| Discovery data | ListenBrainz (similar recordings), MusicBrainz (canonical IDs), Last.fm (similar artists/tracks), AcousticBrainz (frozen but data available) |
| UI | kopuz-inspired sidebar + content, but a dedicated **Discover** page is the USP surface |
| Theming | Token-based CSS, JSON theme files; no Tailwind |
| Auth | Spotify OAuth PKCE → keyring; SC client_id transparent; LB token optional |

Spotify's `/v1/recommendations`, `audio-features`, `audio-analysis` endpoints are deprecated for new apps as of Nov 2024 — explicitly do **not** depend on them. ListenBrainz + Last.fm + AcousticBrainz cover the equivalent ground.

## Crate Layout

```
nira/
├── nira/              shell (window, root, section dispatch)
├── components/        sidebar, bottombar, track-row, mix-card, ctx-menu
├── pages/             home, discover, search, library, settings
├── hooks/             use_player, use_discovery, use_library, use_search, use_settings
├── player/            cpal + symphonia + librespot sink + command channel
├── config/            persisted settings (XDG via directories crate)
├── provider-api/      Provider trait, common types (Track, Artist, TrackUri, ProviderCaps)
├── provider-spotify/  librespot + Spotify Web API
├── provider-soundcloud/ client_id auto-detect, HTTP, stream resolve
├── discovery/         engine: SimilarTo, DailyMix, CrossPlatformBridge modes
└── enrichment/        ListenBrainz / MusicBrainz / Last.fm clients + SQLite cache
```

**Domain rules:**

- Domain crates do not import other domain crates.
- `discovery` may import `provider-api` and `enrichment`, never a concrete `provider-*`.
- `enrichment` is read-only external data — no UI, persistence only as cache.
- Provider asymmetry (SC reposts, Spotify playlists) lives **outside** the trait; UI queries `ProviderCaps` to render conditional surfaces.

## Provider Trait (Sketch)

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn caps(&self) -> ProviderCaps;
    async fn search(&self, q: &Query) -> Result<SearchResults>;
    async fn track(&self, uri: &TrackUri) -> Result<Track>;
    async fn artist(&self, uri: &ArtistUri) -> Result<Artist>;
    async fn resolve_stream(&self, uri: &TrackUri) -> Result<StreamHandle>;
}
```

`StreamHandle` is opaque — internally either a librespot session sink (Spotify) or an HTTP byte stream into symphonia (SoundCloud). The mixer in `player/` consumes both shapes.

## Discovery Engine

**Modes for v1 (all on-demand):**

| Mode | Input | Source | USP |
|------|-------|--------|-----|
| `SimilarTo(track)` | Track seed | ListenBrainz `similar-recordings` + Last.fm tags + AcousticBrainz features | **Cross-platform output**: Spotify seed → SC results possible |
| `DailyMix(seeds = top_artists)` | User listening history | Last.fm similar artists + LB playlists | Baseline feature |
| `CrossPlatformBridge(track)` | Track on one platform | MBID lookup → resolve on the other | "Same track, other platform" explicitly |
| `ListenBrainzWeekly` | User LB token (optional) | LB Weekly Jams / Weekly Exploration | Bonus, open-source FYP |

**Data flow for `SimilarTo`:**

```
seed → (MBID resolve if missing)
     → ListenBrainz similar_recordings(mbid)
     → Last.fm similar_tracks(artist, title)
     → merge + rerank
     → parallel resolve on Spotify + SoundCloud
     → DiscoveryResult { mbid, spotify_uri, sc_uri, score, rationale }
```

UI renders a result with a provider badge `[S]/[SC]/[S+SC]` and a "Play on…" split button.

**Caching.** `enrichment` keeps a SQLite cache keyed on `(source, query_hash)` with TTL. Avoids rate-limit pain and makes repeat queries instant.

**Ranking for MVP.** Weighted sum of LB confidence + Last.fm match score + "available on Spotify" bonus. Audio-feature distance (AcousticBrainz energy/tempo/key) as optional multiplier when data exists. No ML, no user-feedback learning in v1.

**Out of scope for v1:** own audio-feature extraction; user feedback loop / skip-rate learning; social/friend graph.

## Auth

| Provider | Flow | UX |
|----------|------|----|
| Spotify | OAuth PKCE with `127.0.0.1:PORT` callback → token in `keyring` | Settings → "Connect Spotify" → browser tab → done |
| SoundCloud | `client_id` regex-extracted from SC web-player JS at startup; cached, refreshed on 401 | Transparent |
| ListenBrainz | Optional user token for personal weekly playlists | Settings only |
| Last.fm | nira-owned API key, no user login needed for similar endpoints | Transparent |
| MusicBrainz | No auth, polite User-Agent | Transparent |

Credentials sit in `keyring` (Linux Secret Service / macOS Keychain / Windows Credential Manager). Non-sensitive state (SC client_id cache, MB UA) in `config.json`.

## UI Surfaces

- **Sidebar**: Home, Search, **Discover** (USP), Library, Settings.
- **Home**: generated mix cards on demand (refresh button), recently played, optional "Spotify Daily Mix" passthrough.
- **Discover** (USP page): seed drop-zone, mode selector, result grid with provider badges and split "Play on…" button. Rationale tooltip exposes *why* a track was recommended.
- **Search**: aggregated across both providers, filter chip per provider.
- **Library**: liked / followed artists / playlists, provider-aggregated.
- **Settings**: provider connections, themes, cache management.
- **Bottombar**: now-playing + transport.

**Three UI idioms** that make discovery-first visible:

1. Provider badge on every track (`[S]/[SC]/[S+SC]`).
2. Split "Play on…" button when a track exists on both platforms (default → Spotify if Premium-available).
3. Rationale tooltip on discovery results — surfaces the algorithm so it feels less like a black box.

**Themes.** CSS-variable tokens; JSON theme files at `~/.config/nira/themes/*.json`. Default theme = current `main.css` (dark, gold accent).

## Phasing

- **Phase 0** — `provider-api` crate + player skeleton (command channel + cpal output + test-tone source); `hooks::use_player`; `bottombar` wired in.
- **Phase 1** — `provider-soundcloud` first (no OAuth, fast first-audio moment). Search + play in `pages::search`.
- **Phase 2** — `provider-spotify` with OAuth PKCE + librespot in-process.
- **Phase 3** — `discovery/` MVP with `SimilarTo` ✓ and `CrossPlatformBridge` ✓; `enrichment/` with LB+MB+Last.fm clients ✓ and a disk-backed JSON cache ✓ (SQLite deferred — JSON cache satisfies the TTL + keyed-lookup intent at our scale).
- **Phase 4+** — Library page, Spotify Daily-Mix passthrough, ListenBrainz Weekly, theming JSON loader, MPRIS, Discord RPC, scrobble-out, local files.

## Risks & Non-Goals

**Risks:**
- librespot is unofficial; Spotify can break it at any time. Mitigation: keep Spotify behind the Provider trait so a future Connect-only fallback path is feasible.
- SC client_id auto-detect is gray-area and can break with SC web rewrites. Mitigation: graceful degradation, the app still works with Spotify-only.
- Rate limits on LB / Last.fm / MB. Mitigation: aggressive caching, polite User-Agent, batch where possible.

**Non-goals for v1:**
- Web or mobile build.
- Streaming-mainstream UX clone (Spotify-clone). The whole point is to offer a different lens.
- Own theme editor; one good default + JSON-loadable themes is enough.
- Local files (deferred).
- ML / personalised re-ranking (deferred).
