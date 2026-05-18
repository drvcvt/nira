# nira Home — Activity-first Dashboard

**Status:** Approved 2026-05-16
**Scope:** Replaces the current `pages/src/home.rs` placeholder. First
substantive Home implementation.

## Goal

Home is the surface a user sees on app launch. It shows what they have
been doing recently across providers — a "logbook" that reinforces
continuity between sessions, without overlapping the roles of Discover
(seed-driven recommendations) or Library (complete liked-songs view).

## Sections

Three equal-weight sections, stacked top-to-bottom inside the standard
`section.page` shell:

1. **Recently played** — horizontal row, 8 covers. Sourced from a local
   play-log written on every successful `Player::play_*` call.
2. **Recently liked** — horizontal row, 8 covers. Sourced from the
   existing Spotify Liked Songs disk cache, sorted by `added_at`.
3. **Listened lately** — vertical list, 10 lines. Each line: cover +
   title/artist + relative timestamp + provider colour dot. Sourced
   from ListenBrainz `/1/user/<user>/listens` when an LB token and
   username are configured.

Each section degrades to its own empty / error state independently.

## Data sources

### Local play-log

- File: `~/.cache/nira/history.jsonl` (XDG cache dir).
- Format: append-only JSON Lines, one entry per line.
- Entry shape:
  ```rust
  HistoryEntry {
      track_uri: String,
      provider: String,        // ProviderId label
      title: String,
      artist: String,
      cover_url: Option<String>,
      source_label: String,
      played_at: DateTime<Utc>,
  }
  ```
- Cap: 500 entries. Rotation by full file rewrite when the cap is
  exceeded, not on every append (cheap append is the common case).
- Owner: new `player::history` module inside the `player/` crate
  (tightly coupled to play events; not a separate crate).
- Recording trigger: every successful `play_bytes` / `play_spotify`
  call appends one entry derived from the current `NowPlaying`.

### Spotify Liked Songs `added_at`

- Extend `provider_api::Track` with `added_at: Option<DateTime<Utc>>`.
  Optional because not every provider exposes the concept.
- The Spotify provider deserialises `added_at` from `/me/tracks` items
  and populates the field. Existing on-disk caches need invalidation —
  bump the `LikedDiskCache` schema by adding a `version: u32` field
  and treat missing/old version as no-cache.
- `hooks::use_library` exposes a derived `recently_liked` signal: the
  liked tracks sorted by `added_at` desc, truncated to 8.

### ListenBrainz listens

- New method on `EnrichmentClient`:
  `lb_user_listens(username: &str, limit: u32) -> Vec<Listen>`.
- Listen shape:
  ```rust
  Listen {
      mbid: Option<String>,
      title: String,
      artist: String,
      listened_at: DateTime<Utc>,
      source: Option<String>,   // listening_from when present
  }
  ```
- Cached per `(username, limit)` in the existing `TtlCache`, TTL 5 min.
- Needs the LB **username** (not the token) to fetch. New config field
  `listenbrainz_username: Option<String>`. Settings page gets a second
  input next to the token.

## Architecture

### New modules
- `player/src/history.rs` — `History` struct: `record(entry)`,
  `recent(n) -> Vec<HistoryEntry>`, internal append + prune logic.
- `hooks/src/use_history.rs` — `UseHistory { entries: Signal<Vec<HistoryEntry>> }`,
  polling reactor that re-reads on focus / 30 s tick.
- `hooks/src/use_listenbrainz.rs` —
  `UseListenBrainzFeed { listens, is_loading, error }`, fetches on
  mount, retries on focus.

### Modifications
- `player/src/lib.rs` — `Player` owns an `Arc<History>`. Successful
  `play_bytes` and `play_spotify` calls trigger
  `history.record(entry_from_now_playing())`.
- `provider-api/src/lib.rs` — `Track::added_at` field.
- `provider-spotify/src/lib.rs` — parse `added_at` from
  `SavedTrackItem`; flow through to cache.
- `hooks/src/use_library.rs` — `LikedDiskCache` schema bump; derived
  `recently_liked` truncated/sorted signal.
- `enrichment/src/listenbrainz.rs` — `lb_user_listens`.
- `config/src/lib.rs` — `listenbrainz_username: Option<String>` field.
- `pages/src/settings.rs` — username input next to the token.
- `pages/src/home.rs` — full rewrite, three section components.

### Workspace deps
- `chrono = { version = "0.4", features = ["serde"] }` added to
  `Cargo.toml` workspace.dependencies. Used by `provider-api`,
  `player`, `enrichment`, `hooks`.

## Empty / error state matrix

| Section | Empty state | Error state |
|---|---|---|
| Recently played | "No plays yet — hit play in the transport bar." | Disk read error → log + show empty. |
| Recently liked | "Connect Spotify in Settings to see your liked songs." | "Couldn't load liked songs: {err}" |
| Listened lately | "Add a ListenBrainz token + username in Settings to surface your scrobble history." | "Couldn't reach ListenBrainz: {err}" |

## Out of scope (deferred)

- SoundCloud "likes" feeding Recently liked.
- "Find similar" per-row CTAs (would re-implement Discover entry).
- Animations / state-transition motion.
- User-configurable item counts.
- Auto-deriving the LB username from the token (LB API limitation —
  we collect it explicitly).

## Known constraints

- Workspace build is currently broken (librespot 0.8 + vergen). Code
  in this design will compile incrementally with `cargo check -p <crate>`
  for crates outside the librespot chain, but full-app verification
  via `dx serve` waits for the vergen patch to land.
- The LB listens endpoint needs the **username** for path construction;
  the user token only gates scrobble submission. Hence the second
  config field.
