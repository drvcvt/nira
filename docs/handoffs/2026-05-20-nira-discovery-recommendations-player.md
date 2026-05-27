# Handoff: nira discovery, recommendations, player polish

Date: 2026-05-20
Project: `/home/mt/projects/nira`

## Goal / current status

The user wants nira to become daily-drivable before Jellyfin/local files. Current focus is player stability, queue/playback UX, SoundCloud-native discovery/radio quality, and Spotify-like retriggerable recommendations.

Current status:

- Working tree is dirty and uncommitted.
- nira is currently running as PID `34551`, but that binary is now stale relative to the latest Aegis-style Explore/For You changes.
- Runtime log path: `/tmp/nira.log`.
- Latest start command used `GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1 cargo run -p nira` before the newest Explore port.
- Full Rust verification passed after the latest recommendation changes:
  - `cargo fmt --check`
  - `cargo check --workspace`
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets`
- Current UI is available for testing, but the newest Aegis-style For You UI requires a restart. Visual/manual verification is pending.

## Files changed

### Global search overlay / settings cleanup

- `pages/src/search_overlay.rs`
  - Added global search overlay.
  - Results are playable and support context menu.
  - Overlay closes after play.
- `nira/src/main.rs`
  - Added document-level JS hotkeys for search overlay (`Ctrl/Cmd+F`, `Alt+Space`, `Esc`) via hidden buttons.
  - App installs the overlay in the root shell.
- `components/src/searchbar.rs`
  - SearchBar supports mount autofocus.
- `components/src/sidebar.rs`, `components/src/lib.rs`
  - Removed Search as a sidebar/top-level section.
- `pages/src/settings.rs`
  - Removed old Playback settings tab.
  - Added Discovery source controls.

### Queue / playback stability

- `hooks/src/queue.rs`
  - Queue stop now clears entries/current/loading and stops player cleanly.
  - Added `load_generation` stale-load guard.
  - Added page-facing `play_context(tracks, idx)` and `play_track(track)`.
  - Added shuffle and repeat state:
    - `shuffle_enabled`
    - `repeat_mode: Off | All | One`
  - Shuffle applies at queue-context level so Search/Discover/Library/etc. inherit it.
  - Repeat-One only repeats on natural track-end; manual Next still advances.
  - Repeat-All wraps Next/Previous and auto-end.
  - SoundCloud resolve/download errors are shortened for UI and can skip unavailable tracks in longer queues.
- `components/src/bottombar.rs`
  - Added queue popover with current queue, current row highlight, row click playback, context menu, clear button, and shuffle/repeat status chips.
  - Added Shuffle and Repeat transport buttons.
- `nira/src/mpris_bridge.rs`, `player/src/lib.rs`
  - MPRIS Stop routes through queue/player stop request.
- `hooks/src/use_player.rs`, `config/src/lib.rs`, `nira/src/main.rs`
  - Volume persistence added; default volume is `0.8`.
- `player/src/history.rs`, `player/src/lib.rs`, `pages/src/home.rs`
  - History records exact `track_uri`; Recently Played resolves exact URI before search fallback.

### Unified playable surfaces

- `pages/src/parts.rs`
  - Added shared playable helpers/components:
    - `PlayableLi`
    - `PlayableButton`
    - `open_track_context`
    - `provider_badge_class`
    - `format_duration`
- Migrated playable rows/cards in:
  - `pages/src/search_overlay.rs`
  - `pages/src/search.rs`
  - `pages/src/library.rs`
  - `pages/src/album.rs`
  - `pages/src/artist.rs`
  - `pages/src/discover.rs`
  - `pages/src/home.rs`

### Discovery v2 / Aegis-style radio

- `discovery/src/lib.rs`
  - Audited Aegis: the good path was SoundCloud-native `tracks/{id}/related`, preserving SoundCloud order.
  - nira now treats SoundCloud `/related` as the core list.
  - Exact SoundCloud related tracks are preserved; no blind SoundCloud text re-search for those candidates.
  - Last.fm and ListenBrainz act as support/fallback:
    - They can lightly boost matching SoundCloud results.
    - They only fill the list when SoundCloud has too few usable results.
  - ListenBrainz remains opt-in by config.
  - Filtering/dedupe added for:
    - seed reuploads
    - remix/edit/slowed/reverb/nightcore/cover/instrumental/live variants when the seed is not itself a variant
    - canonical title duplicates
  - Added tests for canonical title cleanup, variant filtering, seed reupload detection, SC candidate merge protection, and support-source dedupe/merge.
- `config/src/lib.rs`
  - Added persisted discovery source booleans:
    - `discovery_soundcloud` default `true`
    - `discovery_lastfm` default `true`
    - `discovery_listenbrainz` default `false`
  - Added test for Aegis-style defaults.
- `hooks/src/use_discovery.rs`, `hooks/src/lib.rs`
  - Discovery source prefs exposed and wired into `DiscoveryEngine`.
- `pages/src/discover.rs`
  - Discover banner now reflects source prefs (`sc/lb/lf on/off/no key`).

### For You / recommendations

- `hooks/src/use_recommendations.rs`
  - Reworked the initial 3-card prototype into an Aegis-style Explore model.
  - Exposes shelves, quick tiles, and Daily Mix cards.
  - Shelves now include:
    - `Made for you` from related tracks across recent plays + likes.
    - `Because you played` using exact SoundCloud `/related` when a SC URI exists.
    - `From your artists` using recent uploads from saved SoundCloud artist IDs.
    - `From your likes` as a rotating saved-track slice.
    - `Trending now` from SoundCloud charts.
    - Genre chart shelves for Electronic, Hip-Hop/Rap, House, Techno, Ambient, DnB, Indie.
  - Daily Mixes now build up to 4 Aegis-like mix cards from artist clusters.
  - `Reroll mixes` now targets the daily-mix group correctly.
  - Dedupes by URI and normalized artist/title.
  - Tests cover dedupe, seed extraction, SC-prioritized clusters, and chart-row plan creation.
- `hooks/src/lib.rs`
  - Exports `RecommendationShelf`, `RecommendationMix`, `RecommendationTile`, `UseRecommendations`, and `use_recommendations`.
- `pages/src/home.rs`
  - Replaced the boxed 3-card For You layout with Aegis-like Explore UI:
    - greeting block
    - Shuffle Explore hero
    - quick tiles
    - Daily Mix card scroller
    - horizontal shelf rows with Play/Reroll actions
  - Tracks reuse shared playable cards and queue context.
- `nira/assets/main.css`
  - Added Aegis-like Explore/Home styling for greeting, hero, quick tiles, mix mosaics, horizontal shelf rows, loading skeletons, queue popover, search overlay, discovery/settings UI, and related surfaces.

### Provider / enrichment / misc

- `provider-spotify/src/lib.rs`
  - Spotify caps now mark `ProviderCaps.playable = true`.
- `provider-soundcloud/src/lib.rs`
  - Added SoundCloud chart support via `/charts?kind=trending&genre=soundcloud:genres:<slug>` for Aegis-like genre shelves.
  - Added public `user_tracks()` for recent SoundCloud artist uploads.
  - `artist_top_tracks()` now reuses `user_tracks()`.
  - Existing `/tracks/{id}/related` remains the preferred exact-radio/recommendation source.
- `enrichment/src/*`
  - Last.fm/ListenBrainz/cache wiring touched during discovery-source work.
- `player/src/spotify_backend.rs`
  - Minor playback/backend changes present in current diff.

## Files inspected

Important inspected files:

- `/home/mt/projects/aegis-player/src-tauri/src/providers/soundcloud.rs`
  - `fetch_related_to_track`
  - `fetch_related_to_likes`
  - `fetch_daily_mixes`
- `/home/mt/projects/aegis-player/src-tauri/src/commands/providers.rs`
  - `soundcloud_related_to_track`
- `discovery/src/lib.rs`
- `hooks/src/queue.rs`
- `hooks/src/use_featured.rs`
- `hooks/src/use_history.rs`
- `hooks/src/use_library.rs`
- `pages/src/home.rs`
- `components/src/bottombar.rs`
- `nira/assets/main.css`

## Key decisions / assumptions

- Do not work on Jellyfin/local files yet.
- Never auto-run `dx serve` in this project; it repeatedly opens windows.
- `cargo run -p nira` is OK only when the user explicitly asks to start/test.
- Search is now a global overlay, not a sidebar tab.
- Overlay style should remain dim-only; no blur/fake glass/noise.
- Discovery/radio should be SoundCloud-native first, matching Aegis feel.
- Last.fm is useful as support/fallback; ListenBrainz is opt-in because it can drift broad/popular.
- Playable behavior should stay centralized through shared playable components and `queue.play_context`.
- Shuffle/repeat are queue-level concerns, not page-level concerns.
- The user is currently testing live while listening; avoid unnecessary restarts.

## Commands run and results

Recent relevant commands:

- `cargo fmt --check && cargo check --workspace && cargo test --workspace && cargo clippy --workspace --all-targets` — passed after Discovery v2, the first For You pass, and the later Aegis-style Explore port.
- `cargo test -p hooks` — passed after adding recommendation tests.
- `cargo test -p discovery` — passed after adding discovery-policy tests.
- `GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1 cargo run -p nira` — started current nira process.
- `pgrep -a -x nira` — current PID is `34551`.

Earlier notable non-nira system commands:

- `hyprpm update`, `hyprpm reload`, `hyprctl plugin list` — dynamic cursor plugin now loaded.
- Cursor theme was changed system-wide to RetroSmart; do not keep modifying Hyprland cursor setup unless user asks.

## Open blockers / risks

- Working tree is large and dirty; nothing is committed.
- The currently running nira PID `34551` is stale; restart is needed to see the Aegis-style For You/Explore shelves.
- Visual/user validation of the new Aegis-style UI is pending.
- Recommendations currently depend on Home-mounted history/liked signals; recommendations may be sparse until the user has enough plays/likes cached.
- `use_history()` only samples 8 recent entries; richer recommendations may need a deeper history API.
- Daily Mix cards are generated live from clusters and are not yet persisted/cached daily.
- Current recommendation loading makes several SoundCloud chart/related calls on Home mount; watch latency/logs.
- SoundCloud may still return unavailable tracks; queue skip policy handles some cases, but user testing may reveal more edge cases.

## Exact next steps

1. Restart nira when the user is ready so the stale PID `34551` picks up the Aegis-style Explore/For You changes.
2. Ask for qualitative feedback on:
   - Discover result quality
   - Song Radio quality
   - For You shelf quality
   - Queue/shuffle/repeat behavior
3. If For You quality is acceptable, improve persistence:
   - Cache generated shelves in nira cache dir.
   - Add daily refresh window and manual reroll invalidation.
4. If For You quality is weak, improve seed selection:
   - Use deeper play history, not only 8 recent rows.
   - Weight recent plays and likes.
   - Avoid repeating same artist across shelves.
5. Add better error UX for recommendation shelves:
   - per-shelf partial fallback
   - explicit “not enough listening data” state
6. Consider replacing/merging the older `Featured` hero with the new For You system once stable.
7. Before any final claim, rerun:
   - `cargo fmt --check`
   - `cargo check --workspace`
   - `cargo test --workspace`
   - `cargo clippy --workspace --all-targets`

## Useful resume commands

```sh
git status --short --branch
git diff --stat
git diff -- discovery/src/lib.rs hooks/src/use_recommendations.rs pages/src/home.rs hooks/src/queue.rs components/src/bottombar.rs
pgrep -a -x nira || true
tail -n 120 /tmp/nira.log
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```
