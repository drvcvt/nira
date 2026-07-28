# Liked Recovery, Playlist Removal, and Project Rules Implementation Plan

> **For agentic workers:** Implement this plan task-by-task, one task per commit, and run each task's test before moving on. Track progress with the checkboxes below.

**Goal:** Restore persisted likes in the private build, let users remove imported playlists from their context menu, and make branch and data-safety rules automatic.

**Architecture:** Keep provider compatibility at the persisted `ProviderId` boundary. Reuse the existing playlist store and context-menu infrastructure. Keep shared work public-safe, then cherry-pick exact shared commits into private `master`.

**Tech Stack:** Rust, Dioxus, Serde, Git worktrees, Anvil

## Global Constraints

- `/home/mt/projects/nira` stays on `public`; `/home/mt/projects/nira-private` stays on `master`.
- Never move Qobuz or private configuration into `public`.
- Never overwrite user library files after a parse failure.
- Use `anvil tests`, `anvil check`, and `anvil release`, not local Cargo/Dioxus builds.

---

## Task 1: Restore private liked songs safely

- [ ] Add a failing persistence test in `/home/mt/projects/nira-private/hooks/src/use_likes.rs` for a retired provider ID.
- [ ] Add a Serde fallback in `/home/mt/projects/nira-private/provider-api/src/lib.rs` and preserve Qobuz match arms.
- [ ] Run `anvil tests -- -p hooks saved_track_from_retired_provider_still_loads` and `anvil check`.
- [ ] Verify `/home/mt/.config/nira/likes.json` still contains all 17 entries, commit, then run `anvil release`.

## Task 2: Remove imported playlists by right-click

- [ ] Add one failing import-source test in `/home/mt/projects/nira/hooks/src/use_playlists.rs`.
- [ ] Extend `/home/mt/projects/nira/hooks/src/use_ctx_menu.rs` with an imported-playlist target.
- [ ] Reuse `/home/mt/projects/nira/components/src/ctx_menu.rs` and `/home/mt/projects/nira/pages/src/library.rs` for a confirmed remove action.
- [ ] Run the focused test, `anvil tests`, and `anvil check`, then commit on `public`.

## Task 3: Make project rules automatic

- [ ] Add branch separation, launcher, input-automation, user-data, and public-push safety rules to `/home/mt/projects/nira/AGENTS.md`.
- [ ] Commit the rules separately on `public`.

## Task 4: Integrate, verify, and publish

- [ ] Cherry-pick the public feature and rules commits into private `master`, preserving Qobuz.
- [ ] Run private `anvil tests`, `anvil check`, and `anvil release`.
- [ ] Audit `public` for Qobuz/private leakage and confirm both worktrees are clean.
- [ ] Fetch `origin`, inspect divergence, and push only `public`.
