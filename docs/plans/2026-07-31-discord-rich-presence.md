# Discord Rich Presence Implementation Plan

> **For agentic workers:** Implement this plan task-by-task — one task per commit, run each task's test before moving on. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish Nira playback to Discord as provider-agnostic listening activity with title, artist, album cover, album name, progress, pause, stop, and reconnect behavior.

**Architecture:** Add album text to the existing shared `NowPlaying` snapshot, then project that snapshot into a Discord-only value which cannot contain provider fields. A background thread owns one local Discord IPC client, sends only meaningful changes, clears on stop, and reconnects after Discord restarts.

**Tech Stack:** Rust 2024, `discord-rich-presence` 1.1, existing `PlayerSnapshot`, standard threads and clocks, Anvil checks.

## Global Constraints

- `public` contains only provider-agnostic code; private-provider wiring remains in private `master`.
- Discord must display `Listening to <title>`, never a provider name.
- Public HTTP(S) covers are sent directly; local cover paths fall back to the Discord application icon.
- Paused playback has no moving timestamps; stopped playback clears the activity.
- The bridge must never prevent Nira from starting and must recover when Discord starts or restarts.
- Presence writes must stay within Discord's limit of five updates per 20 seconds.

---

### Task 1: Provider-blind activity projection

**Files:**
- Modify: `player/src/lib.rs`
- Modify: `hooks/src/queue.rs`
- Create: `nira/src/discord_bridge.rs`
- Modify: `nira/src/main.rs`

**Interfaces:**
- Consumes: `player::PlayerSnapshot` and `player::NowPlaying`.
- Produces: private `Presence::from_snapshot(&PlayerSnapshot) -> Option<Presence>` and `Presence::activity(now_ms: i64) -> activity::Activity<'_>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn activity_projection_is_provider_blind() {
    let first = snapshot("Provider A", "provider-a", "a:track:1", false);
    let second = snapshot("Provider B", "provider-b", "b:track:1", false);
    assert_eq!(json(&first), json(&second));
}

#[test]
fn paused_activity_has_no_timestamps() {
    let value = json(&snapshot("Provider B", "provider-b", "b:track:1", true));
    assert!(value.get("timestamps").is_none());
}
```

- [ ] **Step 2: Run the test to verify RED**

Run: `anvil tests`

Expected: FAIL because the Discord bridge projection does not exist.

- [ ] **Step 3: Implement the minimum projection**

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
struct Presence {
    title: String,
    artist: String,
    album: Option<String>,
    cover_url: Option<String>,
    paused: bool,
    position: Duration,
    duration: Option<Duration>,
    playback_id: u64,
}
```

Build `ActivityType::Listening` with `StatusDisplayType::Details`, title in `details`, artist in `state`, a public cover URL in `assets.large_image`, album in `assets.large_text`, and start/end millisecond timestamps only while playing.

- [ ] **Step 4: Run the test to verify GREEN**

Run: `anvil tests`

Expected: PASS, including equality when only provider metadata differs.

- [ ] **Step 5: Commit**

```bash
git add player/src/lib.rs hooks/src/queue.rs nira/src/discord_bridge.rs nira/src/main.rs Cargo.toml nira/Cargo.toml Cargo.lock docs/plans/2026-07-31-discord-rich-presence.md
git commit -m "feat: add provider-blind Discord presence"
```

### Task 2: IPC lifecycle and release verification

**Files:**
- Modify: `nira/src/discord_bridge.rs`

**Interfaces:**
- Consumes: a public Discord Application ID and `Player::snapshot()`.
- Produces: `discord_bridge::start(Player)`, a non-blocking background bridge.

- [ ] **Step 1: Add the failing state-change test**

```rust
#[test]
fn presence_refreshes_only_for_meaningful_changes() {
    let before = Presence::from_snapshot(&snapshot("Provider A", "provider-a", "a:track:1", false));
    let mut later = snapshot("Provider A", "provider-a", "a:track:1", false);
    later.position += Duration::from_millis(500);
    assert_eq!(before, Presence::from_snapshot(&later));
}
```

- [ ] **Step 2: Run the test to verify RED**

Run: `anvil tests`

Expected: FAIL while the projection still treats every position poll as a new activity.

- [ ] **Step 3: Implement the worker**

Poll every 500 ms. Connect lazily, retry after 5 seconds without log spam, force one send after reconnect, send on track/pause/metadata changes, clear once on stop, and rebuild timestamps when position drift shows a seek. Use a compile-time `NIRA_DISCORD_APPLICATION_ID` only until the public Nira Application ID is supplied, then replace it with the public numeric constant.

- [ ] **Step 4: Verify both branches**

Run on `public`: `anvil tests`, `anvil check`, `anvil release`.

Cherry-pick the exact public commit into the private worktree, then run: `anvil tests`, `anvil check`, `anvil release`.

Expected: all commands PASS, private-provider code remains present, and the installed launcher targets the private release.

- [ ] **Step 5: Live smoke test and publish**

With Discord desktop running, play, pause, seek, resume, stop, restart Discord, and play a private-provider track in private Nira. Verify title, artist, cover, album tooltip, progress, frozen pause, cleared stop, automatic reconnect, and no visible provider name. Push only `public`; never push private `master`.
