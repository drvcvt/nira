# Settings Provider Split and Equalizer Implementation Plan

> **For agentic workers:** Implement this plan task-by-task — one task per commit, run each task's test before moving on. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clean up Listen Together, split provider credentials from Music settings, make the session indicator a squircle, and add a real toggleable three-band equalizer.

**Architecture:** Keep the current settings components and split their rendered sections without introducing a new settings framework. Add one lock-free DSP control shared by Rodio sources and the existing librespot sink, with persisted enable state and three fixed-frequency gain values.

**Tech Stack:** Rust 2024, Dioxus 0.7, rodio 0.22, librespot 0.8, existing CSS tokens, Anvil tasks.

## Global Constraints

- `/home/mt/projects/nira` stays on public-safe `public`.
- Use `anvil tests`, `anvil check`, `anvil dev`, and `anvil release` for resource-heavy work.
- No new dependencies.
- Keep local music under Music; external credentials live under Providers.
- Listen Together is the first Music card.
- Equalizer gains are limited to -6 dB through +6 dB.

---

### Task 1: Settings information architecture and controls

**Files:**
- Modify: `pages/tests/ui_regressions.rs`
- Modify: `pages/src/settings/mod.rs`
- Modify: `pages/src/settings/connections.rs`
- Modify: `nira/assets/css/settings.css`
- Modify: `nira/assets/css/base.css`
- Modify: `preview/settings.html`

**Interfaces:**
- Consumes: existing `SettingsCard`, `use_config`, `use_together`, and settings CSS tokens.
- Produces: `MusicSettings`, `ProviderSettings`, a Providers tab, styled session-code inputs, and squircle session chrome.

- [x] **Step 1: Write the failing UI contract test**

```rust
#[test]
fn settings_separate_music_from_providers_and_style_sessions() {
    let settings = include_str!("../src/settings/mod.rs");
    let connections = include_str!("../src/settings/connections.rs");
    let settings_css = include_str!("../../nira/assets/css/settings.css");
    let base_css = include_str!("../../nira/assets/css/base.css");
    assert!(settings.contains("SettingsTab::Providers"));
    assert!(settings.contains("MusicSettings {}"));
    assert!(settings.contains("ProviderSettings {}"));
    assert!(connections.contains("settings-input listen-together-code"));
    assert!(settings_css.contains(".listen-together-code"));
    assert!(base_css.contains(".together-indicator {"));
    assert!(base_css.contains("border-radius: var(--rs);"));
}
```

- [x] **Step 2: Run the test and verify RED**

Run: `anvil tests`

Expected: FAIL because Providers, split components, and Listen Together styles do not exist yet.

- [x] **Step 3: Implement the minimal split and styles**

```rust
enum SettingsTab { Music, Providers, Theme, Discovery, Data }

match active {
    SettingsTab::Music => rsx! { MusicSettings {} LibrarySettings {} },
    SettingsTab::Providers => rsx! { ProviderSettings {} },
    // existing tabs unchanged
}
```

Render Listen Together before Discord in `MusicSettings`; leave Spotify, SoundCloud, and ListenBrainz in `ProviderSettings`. Add `class: "settings-input listen-together-code"` to both session-code inputs and use `var(--rs)` on the fixed session indicator.

- [x] **Step 4: Run the test and verify GREEN**

Run: `anvil tests`

Expected: PASS.

### Task 2: Persisted three-band equalizer DSP

**Files:**
- Create: `player/src/equalizer.rs`
- Modify: `player/src/lib.rs`
- Modify: `player/src/spotify_backend.rs`
- Modify: `config/src/lib.rs`
- Modify: `hooks/src/use_player.rs`
- Modify: `nira/src/main.rs`

**Interfaces:**
- Consumes: rodio `Source`, librespot `Sink`, `AppConfig`, and `UsePlayer`.
- Produces: `EqualizerControl::set(enabled, [low, mid, high])`, `EqualizedSource`, `EqualizedSink`, and `UsePlayer::set_equalizer`.

- [x] **Step 1: Write failing DSP and persistence tests**

```rust
#[test]
fn disabled_equalizer_is_transparent() {
    let control = EqualizerControl::new(false, [6.0, -6.0, 3.0]);
    let mut processor = EqualizerProcessor::new(control);
    assert_eq!(processor.process(0.25, 44_100, 2), 0.25);
}

#[test]
fn missing_equalizer_config_defaults_flat_and_off() {
    let cfg: AppConfig = serde_json::from_str("{}").unwrap();
    assert!(!cfg.equalizer_enabled);
    assert_eq!(cfg.equalizer_bands, [0.0; 3]);
}
```

- [x] **Step 2: Run the tests and verify RED**

Run: `anvil tests`

Expected: FAIL because equalizer types and config fields are absent.

- [x] **Step 3: Implement minimal lock-free DSP wiring**

```rust
pub fn set_equalizer(&self, enabled: bool, bands: [f32; 3]) {
    self.equalizer.set(enabled, bands.map(|gain| gain.clamp(-6.0, 6.0)));
}
```

Use three fixed RBJ peaking filters at 100 Hz, 1 kHz, and 10 kHz. Wrap every Rodio source before the visualizer tap and wrap the existing librespot sink builder so Spotify packets pass through the same processor.

- [x] **Step 4: Run tests and verify GREEN**

Run: `anvil tests`

Expected: PASS.

### Task 3: Equalizer settings UI and final verification

**Files:**
- Modify: `pages/tests/ui_regressions.rs`
- Modify: `pages/src/settings/connections.rs`
- Modify: `nira/assets/css/settings.css`
- Modify: `preview/settings.html`

**Interfaces:**
- Consumes: `UsePlayer::set_equalizer(bool, [f32; 3])` and persisted `AppConfig` fields.
- Produces: one on/off control and Low/Mid/High native range inputs in the Music tab.

- [x] **Step 1: Extend the failing UI contract**

```rust
assert!(connections.contains("Equalizer"));
assert!(connections.contains("set_equalizer"));
assert!(settings_css.contains(".equalizer-grid"));
```

- [x] **Step 2: Run the test and verify RED**

Run: `anvil tests`

Expected: FAIL because the equalizer card is not rendered.

- [x] **Step 3: Implement the minimal UI**

Use the existing `source-toggle` pattern for enable state and native `input type="range"` controls with visible dB output. Persist through `UsePlayer::set_equalizer` on every input event.

- [x] **Step 4: Verify code and both themes**

Run: `anvil tests`, `anvil check`, `anvil release`.

Serve `preview/settings.html`, capture dark and light screenshots, and check input geometry, squircle radii, grayscale hierarchy, overflow, and both themes.
