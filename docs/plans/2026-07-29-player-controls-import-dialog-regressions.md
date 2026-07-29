# Player Controls and Import Dialog Regression Fix Plan

> **For agentic workers:** Implement this plan task-by-task, run each task's regression check before moving on, and keep the shared commits public-safe.

**Goal:** Keep the playlist importer above the player, render clean contained checkmarks, and make seek/volume state react on the input event.

**Architecture:** Fix the importer in CSS because the failure is a stacking-context and native-control rendering issue. Fix playback timing in the shared player hook, then let both seek surfaces call it from `oninput` so the engine and every subscriber see the optimistic value before the poller runs.

**Tech Stack:** Rust, Dioxus, CSS, Anvil

## Global Constraints

- The public worktree remains on `public`.
- The private worktree remains on `master`.
- No private provider code or wiring enters `public`.
- Run heavy checks and builds through the named Anvil tasks.
- Do not push until the user approves the installed build.

---

### Task 1: Lock in the regressions

**Files:**
- Create: `pages/tests/ui_regressions.rs`

**Interfaces:**
- Consumes: the existing CSS and Rust UI sources through `include_str!`
- Produces: two source-level regression checks with no runtime dependencies

- [ ] **Step 1: Add failing checks**

```rust
#[test]
fn playlist_import_dialog_owns_top_layer_and_checkbox_glyph() {
    let css = include_str!("../../nira/assets/css/library.css");
    assert!(css.contains(".content:has(.yt-downloader.open)"));
    assert!(css.contains(".playlist-import-row input:checked::before"));
}

#[test]
fn player_controls_publish_each_input_before_the_poller() {
    let bottombar = include_str!("../../components/src/bottombar.rs");
    let cover = include_str!("../../components/src/cover.rs");
    let player = include_str!("../../hooks/src/use_player.rs");
    let deferred_seek = ["if !*", "scrub_dragging.peek()"].concat();
    let reflected_seek = ["snapshot.write().", "position = target;"].concat();
    let reflected_volume = ["snapshot.write().", "volume = v;"].concat();

    assert!(!bottombar.contains(&deferred_seek));
    assert!(!cover.contains(&deferred_seek));
    assert!(bottombar.contains("player.seek(target);"));
    assert!(cover.contains("player.seek(target);"));
    assert!(player.contains(&reflected_seek));
    assert!(player.contains(&reflected_volume));
}
```

- [ ] **Step 2: Verify RED**

Run: `anvil tests`

Expected: both new tests fail against the current stacking, native checkbox, deferred seek, and polled player snapshot.

### Task 2: Fix importer layering and checkbox rendering

**Files:**
- Modify: `nira/assets/css/library.css`
- Test: `pages/tests/ui_regressions.rs`

**Interfaces:**
- Consumes: `.content`, `.yt-downloader.open`, and the existing playlist row checkbox
- Produces: a temporary content stacking level above `.player` only while the modal is open, plus a contained custom check glyph

- [ ] **Step 1: Raise the modal's containing grid item**

```css
.content:has(.yt-downloader.open) { z-index: 11; }
```

- [ ] **Step 2: Replace the WebKitGTK checkbox glyph**

```css
.playlist-import-row input {
  appearance: none;
  display: grid;
  place-content: center;
  overflow: hidden;
  background: var(--raise1);
}

.playlist-import-row input::before {
  content: "";
  width: 8px;
  height: 5px;
  border: solid currentColor;
  border-width: 0 0 2px 2px;
  transform: translateY(-1px) rotate(-45deg) scale(0);
}

.playlist-import-row input:checked {
  color: var(--chip-on-fg);
  background: var(--chip-on);
}

.playlist-import-row input:checked::before {
  transform: translateY(-1px) rotate(-45deg) scale(1);
}
```

- [ ] **Step 3: Run the targeted workspace test**

Run: `anvil tests`

Expected: the importer regression passes; the player timing regression remains red until Task 3.

### Task 3: Publish seek and volume changes immediately

**Files:**
- Modify: `hooks/src/use_player.rs`
- Modify: `components/src/bottombar.rs`
- Modify: `components/src/cover.rs`
- Test: `pages/tests/ui_regressions.rs`

**Interfaces:**
- Consumes: `UsePlayer::seek`, `UsePlayer::set_volume`, and both range `oninput` handlers
- Produces: optimistic player snapshots and input-time seeks

- [ ] **Step 1: Reflect commands into the shared snapshot**

```rust
pub fn seek(&self, target: std::time::Duration) {
    self.player.seek(target);
    let mut snapshot = self.snapshot;
    snapshot.write().position = target;
}

pub fn set_volume(&self, v: f32) {
    let v = v.clamp(0.0, 1.0);
    self.player.set_volume(v);
    let mut snapshot = self.snapshot;
    snapshot.write().volume = v;
    // existing config persistence follows
}
```

- [ ] **Step 2: Seek from every range input event**

In both seek components, keep the local scrub value while the pointer is down, call `player.seek(target)` directly in `oninput`, and leave `pointerup` responsible only for ending the drag hold.

- [ ] **Step 3: Verify GREEN**

Run: `anvil tests`

Expected: the full workspace test task passes.

### Task 4: Ship both local builds without pushing

**Files:**
- No source changes

**Interfaces:**
- Consumes: verified public commits
- Produces: public release artifact, exact cherry-picks on private `master`, and the installed private release

- [ ] **Step 1: Verify and commit `public`**

Audit the diff for private paths, credentials, and provider wiring, then commit only the plan, checks, and shared fixes.

- [ ] **Step 2: Build public**

Run: `anvil check` and `anvil release`

Expected: both tasks succeed and sync the public release bundle back.

- [ ] **Step 3: Cherry-pick into private `master`**

Cherry-pick only the exact shared fix commit or commits and preserve all private provider code when resolving any conflict.

- [ ] **Step 4: Build and install private**

Run from the private worktree: `anvil check` and `anvil release`, then verify the launcher points at the private release artifact.

- [ ] **Step 5: Stop before push**

Tell the user the builds are ready for local verification. Push neither branch until the user explicitly says the result is fine.
