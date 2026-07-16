# Handoff: review-fix rounds 1+2, anvil offload enablement, docs refresh

Date: 2026-07-16
Project: `/home/mt/projects/nira`

## Goal / current status

Fix the confirmed findings from the 2026-07-16 deep review of snapshot
`330b6f1` (two rounds, 13 findings), then wire up anvil offload for nira
per user request, then refresh stale docs.

- Branch `master`. All session work is **committed** (`313ec62`, `25b6adc`,
  `0ca8ff6`, `1d180be`, plus a docs commit after this file).
- Working tree is NOT clean: `hooks/src/taste.rs` (new), `hooks/Cargo.toml`,
  `hooks/src/lib.rs`, `hooks/src/queue.rs`, `hooks/src/use_history.rs`,
  `hooks/src/use_recommendations.rs`, `Cargo.lock` are the **user's own
  in-progress taste-weighting refactor** — do not revert, do not commit
  on his behalf.
- Round-1 fixes verified in the headless rig (Home + search overlay
  screenshots); round-2 badge fix visually confirmed (L/S badges on
  history cards). Settings pills could not be reached in the rig
  (headless seat has no pointer; sway IPC clicks don't reach WebKit) —
  they are a 1:1 port of the previously working monolith code and
  compile clean.
- anvil cargo offload to worker `mini` verified end-to-end
  (`anvil cargo -- check -p provider-api` green).

## Files changed

### Round 1 (`313ec62`) — top review findings
- `hooks/src/matching.rs` — Unicode-aware `match_key` (keeps Cyrillic/CJK),
  empty-key guards, duration-0 now REJECTS instead of accepting; tests
  added/inverted.
- `provider-hires-provider/src/lib.rs` — disc-prefixed filenames `D-NN - Title.ext`
  for disc ≥ 2 (multi-disc delta-skip collision).
- `provider-hires-provider/src/auth.rs` + `hooks` — 401 clears token from memory
  and disk cache (self-healing logged-out state).
- `hooks/src/use_history.rs` — 30 s poll diffs against last state; idle
  ticks no longer re-render Home.
- `pages/src/home/activity.rs` — history-card click resolves entries
  concurrently (`join_all`), failure surfaces via `queue.error`.
- `components`/`pages` (many) — new `TrackCtx` (Arc + pointer-eq) replaces
  `Vec<Track>` props on `PlayableButton`/`PlayableLi` across Home rails,
  stage, search, album, artist, discover, search overlay.

### Round 2 (`25b6adc`) — remaining bugs + regressions
- `provider-spotify/src/lib.rs` — `sp_mid_image` picks the true middle
  image (`len/2`), not blindly the 2nd (2-image case returned 64 px art).
- `pages/src/home/mod.rs` — `badge_class_for`/`badge_glyph_for` know
  the hi-res provider ("Q") and Local ("L").
- `player/src/lib.rs` — `TransportCmd::SeekFailed(String)` (enum lost
  `Copy`); http seek-rebuild failure and sync rebuild failure send it;
  TOCTOU fixed by holding the `rodio_source` read guard across
  check+swap (sound because every play path updates `rodio_source`
  BEFORE touching the sink).
- `hooks/src/queue.rs` — transport-bus consumer maps `SeekFailed` to
  `queue.error` (bottombar toast).
- `hooks/src/use_recommendations.rs` — per-shelf reroll supersede check:
  the offset counter doubles as a per-shelf generation; stale overlapping
  rerolls of the same shelf drop their results.
- `nira/assets/css/home.css` — restored `.mix-card:disabled` block
  (dim + suppress hover lift).
- `nira/assets/css/settings.css` — restored `.settings-pill/.settings-dot/
  .settings-meta-grid` CSS.
- `pages/src/settings/mod.rs` — restored `StatusPill` component.
- `pages/src/settings/connections.rs` — LB card shows "Scrobbling on/off"
  + "Home feed on/off" pills (active config state, not drafts).
- `pages/src/settings/discovery.rs` — "Last.fm needs an API key" pill when
  the source toggle is on but no key is active.

### anvil enablement (`0ca8ff6`, `1d180be` here; `a897c85` in ~/projects/anvil)
- `anvil.toml` (new) — `[project] default_profile = "cargo"`,
  `[defaults] disk = "30G"`; commented ready-to-go `[tasks.dev]`/
  `[tasks.release]` using ephemeral `nix-shell -p dioxus-cli …` (the
  `[tasks]` feature is design-only in anvil, parser rejects it today).
- `~/projects/anvil` commit `a897c85` — rsync excludes lost their
  trailing slashes (`target/` → `target` etc.): slashed patterns only
  match real directories, and nira's `target` is a **symlink** to
  `/mnt/data/targets/nira` (user moved it 2026-07-16 13:51), which was
  synced as a dangling link and broke remote cargo with "Not a
  directory". 137 anvil tests pass; binary installed to `~/.local/bin`.

### Docs refresh (this commit)
- `README.md` — status/table/tree/setup/roadmap updated: local provider +
  the hi-res provider are shipped, CSS is split under `nira/assets/css/`, anvil warning
  replaced with the new offload guidance, the hi-res provider first-run bullet added.
- `docs/hires-provider.md` — 401 self-heal note, disc-prefix layout, Unicode/
  duration-veto matching notes.

## Files inspected

- `player/src/lib.rs` (seek paths, transport bus, play-path ordering),
  `hooks/src/queue.rs` (bus consumer, error signal), old monolith CSS via
  `git show 330b6f1^:nira/assets/main.css`, old settings monolith via
  `git show 330b6f1^:pages/src/settings.rs`, anvil sources
  (`job.rs`/`service.rs`/`main.rs`/`config.rs`) for the exclude fix,
  anvil docs/specs for the `[tasks]` design.

## Key decisions / assumptions

- Provider badges stay visually neutral (grayscale design); identity is
  the glyph text — so the badge fix is glyph/class only, no colors.
- `TransportCmd` bus was the cheapest correct error channel from player
  to UI; no new channel added.
- Reroll race fixed with the existing offset counter as per-shelf
  generation — no new state.
- Discovery page did NOT get the full 3-pill meta-grid back (toggles
  already show on/off); only the lost "needs key" signal was restored.
- anvil: cargo offload is now allowed for nira; dx bundling stays local
  until the nix-shell task path is proven once. mini's nixpkgs has
  dioxus-cli **0.7.9** (= workspace) and webkitgtk_4_1 2.52.5 — version
  risk cleared. Nothing is persistently installed on the worker.
- Memory `no-anvil-builds.md` rewritten accordingly.

## Commands run and results

- `cargo check -p provider-spotify -p player -p hooks -p pages -j4` — clean.
- `cargo test -p hooks -p player -p provider-spotify -p pages` — all green
  (14 hooks incl. new matching tests).
- `scripts/rebuild.sh` (nice, 4 jobs) — bundle rebuilt twice (after each
  round); rig-verified.
- `anvil doctor cargo` — all pass; `anvil cargo -- check -p provider-api`
  — green on mini (1 m 38 s cold, persistent workspace cache).
- `cargo test -p anvil` (in ~/projects/anvil) — 137 passed.
- Disk incident: root hit 100 % mid-session; freed 6.8 G by deleting
  nira's `target/debug/incremental`. User then moved `target` to
  `/mnt/data/targets/nira` (symlink) — root cause of the anvil sync bug.

## Open blockers / risks

- **User WIP in tree:** `hooks/src/taste.rs` + related hook edits
  (taste-weighting refactor) are uncommitted and untested by this
  session. Coordinate before touching `hooks/`.
- Settings pills not visually verified (rig pointer limitation). Risk low.
- The nix-shell dx task path is **untested end-to-end** (first run pulls
  the WebKit closure onto mini and does a cold build).
- the hi-res provider token in `config.json` survives restarts after a 401 by design;
  it is cleared again on first failing call.
- Open review findings (tracked in memory `nira-daily-use-phase.md`):
  Home per-render clones without `use_memo` (`home/mod.rs:41`), sync
  `load_cache` on Home mount, provider dispatch triplication in hooks,
  download capability hardwired to the hi-res provider in UI, 3 duplicate normalizers,
  provider-local non-UTF-8 paths, `rebuild.sh` lacks internal nice/jobs.
- Dioxus signal-scoping warning in `use_local_library` (potential
  use-after-drop) — spawned as task chip `task_deed88a8`.

## Exact next steps

1. If the user asks for review round 3: pick from the open findings list
   above (perf pair on Home first: `use_memo` + async `load_cache`).
2. Optionally prove the remote release build once:
   `anvil build -- nix-shell -p dioxus-cli pkg-config gtk3 webkitgtk_4_1 libsoup_3 glib openssl alsa-lib --run "dx build --desktop -p nira --release"`.
3. When anvil's `[tasks]` feature is implemented, uncomment the task
   blocks in `anvil.toml` → `anvil dev` / `anvil release` work.
4. Leave the user's `taste.rs` refactor alone unless asked.

## Useful resume commands

```sh
git -C /home/mt/projects/nira status --short --branch
git -C /home/mt/projects/nira log --oneline -8
nice -n 10 cargo check --workspace -j4          # local
anvil cargo -- check -p <crate>                  # offloaded to mini
anvil config show cargo && anvil doctor cargo
bash scripts/rebuild.sh                          # local bundle (rig-safe)
```
