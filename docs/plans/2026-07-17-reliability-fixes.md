# Nira Reliability Fixes Implementation Plan

> **For agentic workers:** Implement this plan task-by-task, run each task's focused test before moving on. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent duplicate Nira audio sessions, stale Spotify events, persistence races, poisoned the hi-res provider downloads, stale library scans, invalid the hi-res provider credentials, and missed repeated-track scrobbles.

**Architecture:** Keep each fix at the shared state boundary that owns the invariant. Use standard-library locking and atomic rename, librespot's existing request IDs, the existing Dioxus signals, and one player playback counter; add no dependencies or speculative abstractions.

**Tech Stack:** Rust 2024, Dioxus, librespot 0.8, Tokio, serde_json, lofty.

**Execution status (2026-07-17):** Tasks 1–5 are implemented and their focused
regression tests pass. `git diff --check` passes. The project-wide Anvil gate is
blocked before Nira compilation: `anvil check` cannot find `pango.pc`, while
`anvil dev` stalls in the remote `cargo metadata` step.

## Global Constraints

- Preserve the user's existing changes in `components/src/visualizer.rs` and `player/src/viz.rs`.
- Add no dependencies.
- Do not perform Git commits unless the user explicitly asks.
- Every non-trivial fix starts with a focused failing test.

---

### Task 1: Single-instance and Spotify event ownership

**Files:**
- Modify: `nira/src/main.rs`
- Modify: `player/src/spotify_backend.rs`

**Interfaces:**
- Consumes: `AppConfig::cache_dir()`, `std::fs::File::try_lock`, and `PlayerEvent::get_play_request_id()`.
- Produces: `acquire_instance_lock(path: &Path) -> io::Result<File>` and a Spotify reducer that ignores events from previous play requests.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn second_instance_lock_is_rejected() {
    let first = acquire_instance_lock(&path).unwrap();
    assert!(acquire_instance_lock(&path).is_err());
    drop(first);
}

#[test]
fn stale_stop_does_not_clear_current_track() {
    let mut state = state_for_request(2);
    apply_event(&mut state, stopped_event(1));
    assert!(state.has_track);
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p nira second_instance_lock_is_rejected && cargo test -p player stale_stop_does_not_clear_current_track`

Expected: FAIL because the lock helper and correlated event reducer do not exist.

- [ ] **Step 3: Implement the minimum fixes**

```rust
fn acquire_instance_lock(path: &Path) -> io::Result<File> {
    let file = File::options().create(true).write(true).open(path)?;
    file.try_lock()?;
    Ok(file)
}

if event.get_play_request_id() != state.play_request_id {
    return;
}
```

- [ ] **Step 4: Verify GREEN**

Run the two focused tests again. Expected: PASS.

### Task 2: Ordered, atomic persistence

**Files:**
- Modify: `config/src/lib.rs`
- Modify: `hooks/src/use_likes.rs`
- Modify: `hooks/src/use_playlists.rs`
- Modify: `hooks/src/queue.rs`

**Interfaces:**
- Consumes: existing `AppConfig::atomic_write_json` callers.
- Produces: `AppConfig::atomic_write(path, bytes)` with serialized writes and unique temporary files; `AppConfig::save` routes through the same helper.

- [ ] **Step 1: Write a failing concurrent-write test**

```rust
#[test]
fn concurrent_atomic_writes_do_not_share_a_temp_file() {
    let results = threads_that_write_the_same_path();
    assert!(results.into_iter().all(Result::is_ok));
    assert!(serde_json::from_str::<Value>(&read_to_string(path).unwrap()).is_ok());
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p config concurrent_atomic_writes_do_not_share_a_temp_file`

Expected: FAIL because current writers race on the same `.tmp` path.

- [ ] **Step 3: Implement the minimum fix**

```rust
static WRITE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_ID: AtomicU64 = AtomicU64::new(0);
```

Route config saves through the helper and remove detached writer threads so mutation order is also persistence order.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p config`. Expected: PASS.

### Task 3: the hi-res provider download and secret validation

**Files:**
- Modify: `provider-hires-provider/src/lib.rs`
- Modify: `provider-hires-provider/src/auth.rs`

**Interfaces:**
- Consumes: `AppConfig::atomic_write`, `lofty::read_from_path`, and `reqwest::StatusCode`.
- Produces: unreadable existing downloads are replaced; only `200 OK` and `401 Unauthorized` prove a valid signature.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn unreadable_download_is_not_reusable() {
    assert_eq!(basic_tags_status(&invalid_file), None);
}

#[test]
fn server_errors_do_not_validate_a_secret() {
    assert!(!accepted_secret_status(StatusCode::INTERNAL_SERVER_ERROR));
    assert!(accepted_secret_status(StatusCode::OK));
    assert!(accepted_secret_status(StatusCode::UNAUTHORIZED));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p provider-hires-provider unreadable_download_is_not_reusable server_errors_do_not_validate_a_secret`

Expected: FAIL under the current unreadable-file and status handling.

- [ ] **Step 3: Implement the minimum fixes**

Use `Option<bool>` to distinguish unreadable audio from missing tags, remove an unreadable final file, write downloads via atomic rename, and restrict accepted statuses to `OK | UNAUTHORIZED`.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p provider-hires-provider`. Expected: PASS.

### Task 4: Latest library scan wins

**Files:**
- Modify: `hooks/src/use_local_library.rs`

**Interfaces:**
- Consumes: root-owned Dioxus signals already held by `UseLocalLibrary`.
- Produces: a monotonically increasing scan generation; only the current generation may update tracks, errors, bytes, or scanning state.

- [ ] **Step 1: Write the failing generation test**

```rust
#[test]
fn older_scan_result_is_stale() {
    assert!(!is_current_scan(1, 2));
    assert!(is_current_scan(2, 2));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p hooks older_scan_result_is_stale`

Expected: FAIL because scan generations do not exist.

- [ ] **Step 3: Implement the minimum fix**

Add one private root signal, increment it before every scan, and return without mutating state when completion sees a newer generation.

- [ ] **Step 4: Verify GREEN**

Run the focused test again. Expected: PASS.

### Task 5: Repeated-track scrobble identity

**Files:**
- Modify: `player/src/lib.rs`
- Modify: `hooks/src/scrobble.rs`

**Interfaces:**
- Consumes: the existing `record_now_playing` commit point.
- Produces: `PlayerSnapshot::playback_id`, incremented once per committed playback; scrobbling keys on this ID and ignores loading gaps.

- [ ] **Step 1: Write the failing repeated-track test**

```rust
#[test]
fn repeated_track_with_new_playback_id_resets_watcher() {
    assert!(watcher.start_if_new(10));
    assert!(!watcher.start_if_new(10));
    assert!(watcher.start_if_new(11));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p hooks repeated_track_with_new_playback_id_resets_watcher`

Expected: FAIL because the watcher currently keys only on artist and title.

- [ ] **Step 3: Implement the minimum fix**

Increment an `AtomicU64` at the existing playback commit point, expose it in snapshots, and compare it in `ScrobbleWatcher`.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p hooks repeated_track_with_new_playback_id_resets_watcher`. Expected: PASS.

### Task 6: Workspace verification

**Files:**
- Verify all files above; make no unrelated changes.

- [ ] **Step 1: Format and inspect scope**

Run: `cargo fmt --all -- --check`, `git diff --check`, and `git diff --stat`.

- [ ] **Step 2: Run focused suites**

Run: `cargo test -p config -p player -p provider-hires-provider -p hooks -p nira`.

- [ ] **Step 3: Run the workspace check**

Run: `cargo check --workspace`.

- [ ] **Step 4: Confirm runtime precondition**

Before manual audio testing, ensure only one Nira process and one PipeWire Nira stream remain.
