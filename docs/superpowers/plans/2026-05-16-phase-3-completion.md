# Phase 3 Completion — Last.fm + CrossPlatformBridge

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the gaps between the current discovery surface and the discovery-pivot spec's Phase 3 (`docs/superpowers/specs/2026-05-16-nira-discovery-pivot-design.md`): add a Last.fm candidate source and a dedicated CrossPlatformBridge mode.

**Architecture:** Reuse the existing `EnrichmentClient` + JSON cache pattern (no SQLite migration — the disk-backed JSON cache already satisfies the spec's intent at our scale). Last.fm joins LB+MB as a third method on the same client. The discovery engine grows one new entry point (`cross_platform_bridge`) that resolves a given track on every *other* provider via the existing `Provider::search` trait.

**Tech Stack:** Rust 2024, reqwest, serde, async-trait, futures, existing TtlCache.

**Scope explicitly NOT in this plan (Phase 4+ per spec):**
- DailyMix and ListenBrainzWeekly modes
- AcousticBrainz audio-feature reranking
- Theming JSON loader, Discord RPC

---

## File Structure

**Create:**
- `enrichment/src/lastfm.rs` — Last.fm `track.getSimilar` client (~120 lines)
- `discovery/src/cross_platform.rs` — `CrossPlatformBridge` mode (~100 lines)

**Modify:**
- `enrichment/src/lib.rs` — add `pub mod lastfm`, expose `LastFmSimilar`, add `lastfm_api_key()` getter
- `enrichment/Cargo.toml` — no new deps (reuse reqwest+serde)
- `discovery/src/lib.rs` — add `lastfm_candidates` source, wire into `similar_to`, expose new mod
- `discovery/Cargo.toml` — no new deps
- `hooks/src/use_discovery.rs` — add `bridge()` method
- `hooks/src/lib.rs` — re-export `CrossPlatformMatch`
- `pages/src/discover.rs` — mode toggle (SimilarTo ↔ CrossPlatformBridge)
- `config/src/lib.rs` — `lastfm_api_key: Option<String>` field (env fallback in enrichment)

---

## Decisions Locked In

1. **No SQLite migration.** Existing `enrichment/src/cache.rs` is disk-backed JSON with atomic write + per-read TTL check. Functionally equivalent to "SQLite cache keyed on `(source, query_hash)` with TTL" for our ~10KB working set. Marking spec item satisfied.

2. **Last.fm key sourcing.** Reads in order: `AppConfig.lastfm_api_key` → env `NIRA_LASTFM_API_KEY` → none (skip Last.fm source silently). No Settings UI yet — dev/maintainer sets it once. Last.fm doesn't require a user-account, only an app key.

3. **CrossPlatformBridge resolution.** Text-search-based on `"{artist} {title}"` against each non-seed provider; pick top hit. MBID-based exact match deferred — adds an MB roundtrip per call and v1 doesn't need it.

4. **Rationale strings stay plain text.** Already the pattern. Last.fm contributions show up as `"Last.fm"` in the source list.

---

## Task 1: Last.fm Client (pure parsing)

**Files:**
- Create: `enrichment/src/lastfm.rs`
- Modify: `enrichment/src/lib.rs`

### Step 1.1 — Stub the module + types

- [ ] Create `enrichment/src/lastfm.rs` with:

```rust
//! Last.fm `track.getSimilar` similarity lookups.
//!
//! Uses a single app-owned API key (no user login, no token). The endpoint
//! returns a ranked list of similar tracks with a normalised `match` score
//! in [0.0, 1.0]. Empty if Last.fm has no neighbourhood for the seed.
//!
//! Key sourcing: `AppConfig.lastfm_api_key` → env `NIRA_LASTFM_API_KEY` →
//! none. Discovery skips this source silently when no key is configured.

use serde::Deserialize;

use crate::{EnrichmentClient, EnrichmentError, EnrichmentResult};

const LASTFM_API: &str = "https://ws.audioscrobbler.com/2.0";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastFmSimilar {
    pub title: String,
    pub artist: String,
    /// Normalised in [0.0, 1.0]. Higher = closer neighbour.
    pub score: f32,
}

impl EnrichmentClient {
    /// Returns similar tracks. Empty when no key is configured or Last.fm
    /// has nothing for this seed; Err only on real network/malformed.
    pub async fn lastfm_similar_tracks(
        &self,
        api_key: Option<&str>,
        artist: &str,
        title: &str,
        limit: u32,
    ) -> EnrichmentResult<Vec<LastFmSimilar>> {
        let Some(key) = api_key.filter(|k| !k.trim().is_empty()) else {
            return Ok(Vec::new());
        };
        let cache_key = format!("lastfm:similar:{artist}|{title}|{limit}");
        if let Some(cached) = self.cache().get(&cache_key)
            && let Ok(parsed) = serde_json::from_str::<Vec<LastFmSimilar>>(&cached)
        {
            return Ok(parsed);
        }
        let url = format!(
            "{LASTFM_API}/?method=track.getsimilar&artist={a}&track={t}&limit={limit}&api_key={k}&format=json",
            a = url::form_urlencoded::byte_serialize(artist.as_bytes()).collect::<String>(),
            t = url::form_urlencoded::byte_serialize(title.as_bytes()).collect::<String>(),
            k = url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>(),
        );
        let resp = self
            .http()
            .get(&url)
            .send()
            .await
            .map_err(|e| EnrichmentError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let snippet = resp.text().await.unwrap_or_default();
            return Err(EnrichmentError::Network(format!(
                "Last.fm track.getsimilar -> {status}: {}",
                snippet.chars().take(400).collect::<String>()
            )));
        }
        let raw: LastFmResp = resp
            .json()
            .await
            .map_err(|e| EnrichmentError::Malformed(e.to_string()))?;
        let mapped = parse_similar_tracks(raw);
        let serialised = serde_json::to_string(&mapped).unwrap_or_default();
        self.cache().put(cache_key, serialised);
        Ok(mapped)
    }
}

fn parse_similar_tracks(raw: LastFmResp) -> Vec<LastFmSimilar> {
    let Some(group) = raw.similartracks else {
        return Vec::new();
    };
    group
        .track
        .into_iter()
        .filter_map(|t| {
            // Last.fm sometimes returns `match` as string "0.83" and sometimes
            // as a JSON number. Accept either via untagged Match wrapper.
            let score = t.match_score.as_f32().clamp(0.0, 1.0);
            Some(LastFmSimilar {
                title: t.name?,
                artist: t.artist.and_then(|a| a.name)?,
                score,
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct LastFmResp {
    #[serde(default)]
    similartracks: Option<SimilarTracksGroup>,
}

#[derive(Deserialize)]
struct SimilarTracksGroup {
    #[serde(default)]
    track: Vec<RawTrack>,
}

#[derive(Deserialize)]
struct RawTrack {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    artist: Option<RawArtist>,
    #[serde(default, rename = "match")]
    match_score: MatchScore,
}

#[derive(Deserialize)]
struct RawArtist {
    #[serde(default)]
    name: Option<String>,
}

/// Last.fm flips between number and string forms for `match`. Tolerate both.
#[derive(Default, Deserialize)]
#[serde(untagged)]
enum MatchScore {
    Num(f32),
    Str(String),
    #[default]
    Missing,
}

impl MatchScore {
    fn as_f32(&self) -> f32 {
        match self {
            MatchScore::Num(n) => *n,
            MatchScore::Str(s) => s.parse().unwrap_or(0.0),
            MatchScore::Missing => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_number_match() {
        let raw: LastFmResp = serde_json::from_str(
            r#"{"similartracks":{"track":[{"name":"Heroes","artist":{"name":"Bowie"},"match":0.91}]}}"#,
        )
        .unwrap();
        let out = parse_similar_tracks(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Heroes");
        assert_eq!(out[0].artist, "Bowie");
        assert!((out[0].score - 0.91).abs() < 1e-3);
    }

    #[test]
    fn parses_string_match() {
        let raw: LastFmResp = serde_json::from_str(
            r#"{"similartracks":{"track":[{"name":"X","artist":{"name":"Y"},"match":"0.5"}]}}"#,
        )
        .unwrap();
        let out = parse_similar_tracks(raw);
        assert_eq!(out[0].score, 0.5);
    }

    #[test]
    fn empty_when_no_group() {
        let raw: LastFmResp = serde_json::from_str("{}").unwrap();
        assert!(parse_similar_tracks(raw).is_empty());
    }

    #[test]
    fn skips_entries_missing_artist_or_title() {
        let raw: LastFmResp = serde_json::from_str(
            r#"{"similartracks":{"track":[{"match":1.0},{"name":"X","artist":{"name":"Y"},"match":1.0}]}}"#,
        )
        .unwrap();
        let out = parse_similar_tracks(raw);
        assert_eq!(out.len(), 1);
    }
}
```

- [ ] Modify `enrichment/src/lib.rs` to expose the module + re-export. Find the line:

```rust
pub mod cache;
pub mod listenbrainz;
pub mod musicbrainz;
```

Replace with:

```rust
pub mod cache;
pub mod lastfm;
pub mod listenbrainz;
pub mod musicbrainz;

pub use lastfm::LastFmSimilar;
```

### Step 1.2 — Run the parser tests

- [ ] Run: `command cargo test -p enrichment lastfm --quiet`
- [ ] Expected: 4 passed.

### Step 1.3 — Workspace check

- [ ] Run: `command cargo check --workspace`
- [ ] Expected: clean, no warnings on the new module.

### Step 1.4 — Commit

```bash
git add enrichment/src/lastfm.rs enrichment/src/lib.rs
git commit -m "feat(enrichment): add Last.fm track.getSimilar client"
```

---

## Task 2: Wire Last.fm key into config

**Files:**
- Modify: `config/src/lib.rs`
- Modify: `enrichment/src/lib.rs` (add `lastfm_api_key` field + setter)

### Step 2.1 — Config field

- [ ] In `config/src/lib.rs`, add a new field on `AppConfig` after `listenbrainz_username`:

```rust
    /// Last.fm app-owned API key. Optional — if absent the discovery engine
    /// silently skips the Last.fm candidate source. Falls back to the
    /// `NIRA_LASTFM_API_KEY` env var at startup if this field is empty.
    #[serde(default)]
    pub lastfm_api_key: Option<String>,
```

### Step 2.2 — EnrichmentClient holds the key

- [ ] In `enrichment/src/lib.rs`, change `EnrichmentClient` to carry the key:

```rust
#[derive(Clone)]
pub struct EnrichmentClient {
    http: Client,
    cache: Arc<TtlCache>,
    lastfm_key: Option<String>,
}

impl EnrichmentClient {
    pub fn new() -> EnrichmentResult<Self> {
        Self::with_lastfm_key(None)
    }

    pub fn with_lastfm_key(key: Option<String>) -> EnrichmentResult<Self> {
        let http = Client::builder()
            .user_agent(
                "nira/0.1.0 (https://github.com/dracut/nira; cross-platform music discovery)",
            )
            .build()
            .map_err(|e| EnrichmentError::Network(e.to_string()))?;
        // Config wins over env, both can be missing. Whitespace-only is
        // treated as missing — copy-paste accidents happen.
        let lastfm_key = key
            .filter(|k| !k.trim().is_empty())
            .or_else(|| std::env::var("NIRA_LASTFM_API_KEY").ok())
            .filter(|k| !k.trim().is_empty());
        Ok(Self {
            http,
            cache: Arc::new(TtlCache::new()),
            lastfm_key,
        })
    }

    pub fn lastfm_key(&self) -> Option<&str> {
        self.lastfm_key.as_deref()
    }

    pub(crate) fn http(&self) -> &Client {
        &self.http
    }

    pub(crate) fn cache(&self) -> &TtlCache {
        &self.cache
    }
}
```

### Step 2.3 — Find the EnrichmentClient construction site and pass the key

- [ ] Run: `grep -rn "EnrichmentClient::new" /home/mt/projects/nira/{nira,hooks,discovery}/src/`
- [ ] At each call site, replace `EnrichmentClient::new()` with `EnrichmentClient::with_lastfm_key(cfg.lastfm_api_key.clone())` where `cfg` is the loaded `AppConfig`. If a call site has no access to `AppConfig`, keep `::new()` (env-var-only path).

### Step 2.4 — Workspace check + commit

- [ ] Run: `command cargo check --workspace`
- [ ] Expected: clean.

```bash
git add config/src/lib.rs enrichment/src/lib.rs <any-call-site-files>
git commit -m "feat(enrichment): plumb Last.fm api key via config + env fallback"
```

---

## Task 3: Wire Last.fm into discovery::similar_to

**Files:**
- Modify: `discovery/src/lib.rs`

### Step 3.1 — Add `lastfm_candidates`

- [ ] In `discovery/src/lib.rs`, after `lb_candidates` (around line 270–307), add:

```rust
    /// Last.fm `track.getSimilar` neighbourhood. Empty when no key is
    /// configured or Last.fm has nothing for the seed; Err only on real
    /// network/malformed errors.
    async fn lastfm_candidates(
        &self,
        seed: &SimilarToSeed,
    ) -> Result<Vec<Candidate>, DiscoveryError> {
        if seed.artist.is_empty() || seed.title.is_empty() {
            return Ok(Vec::new());
        }
        let key = self.enrichment.lastfm_key();
        if key.is_none() {
            return Ok(Vec::new());
        }
        let similar = match self
            .enrichment
            .lastfm_similar_tracks(key, &seed.artist, &seed.title, 30)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "Last.fm similar failed; skipping path");
                return Ok(Vec::new());
            }
        };
        Ok(similar
            .into_iter()
            .map(|s| Candidate {
                mbid: None,
                title: s.title,
                artist: s.artist,
                score: s.score.clamp(0.0, 1.0),
                sources: vec!["Last.fm"],
            })
            .collect())
    }
```

### Step 3.2 — Merge Last.fm into `similar_to`

- [ ] Find the `tokio::join!` block at the top of `similar_to` (around line 134–137):

```rust
        let (sc_path, lb_path) = tokio::join!(
            self.sc_candidates(&seed),
            self.lb_candidates(&seed),
        );

        let mut buckets: HashMap<String, Candidate> = HashMap::new();
        for source_set in [sc_path, lb_path].into_iter().flatten() {
```

Replace with:

```rust
        let (sc_path, lb_path, lastfm_path) = tokio::join!(
            self.sc_candidates(&seed),
            self.lb_candidates(&seed),
            self.lastfm_candidates(&seed),
        );

        let mut buckets: HashMap<String, Candidate> = HashMap::new();
        for source_set in [sc_path, lb_path, lastfm_path].into_iter().flatten() {
```

### Step 3.3 — Workspace check + commit

- [ ] Run: `command cargo check --workspace`
- [ ] Expected: clean.

```bash
git add discovery/src/lib.rs
git commit -m "feat(discovery): include Last.fm as third similarity candidate source"
```

---

## Task 4: CrossPlatformBridge mode

**Files:**
- Create: `discovery/src/cross_platform.rs`
- Modify: `discovery/src/lib.rs` (mod + re-export + method)
- Modify: `hooks/src/use_discovery.rs` (expose `bridge()`)
- Modify: `hooks/src/lib.rs` (re-export)
- Modify: `pages/src/discover.rs` (mode toggle)

### Step 4.1 — Define types

- [ ] Create `discovery/src/cross_platform.rs`:

```rust
//! "Same track, other platform" — given one track, find the best playable
//! match on every *other* registered provider.
//!
//! Distinct from `SimilarTo`: this mode does NOT walk a similarity
//! neighbourhood. It's a 1:1 identity bridge. Useful when a user finds a
//! track on SoundCloud and wants the Spotify version (for Connect-cast,
//! offline, or Premium-only features) or vice versa.
//!
//! Resolution is text-search on `"{artist} {title}"`. MBID-exact matching
//! would need an MB roundtrip per call — deferred to a later phase.

use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use provider_api::{Provider, ProviderId, Query, Track};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossPlatformMatch {
    /// The seed track exactly as supplied by the caller.
    pub source: Track,
    /// Best-effort match on Spotify, if Spotify is registered and finds one.
    pub spotify: Option<Track>,
    /// Best-effort match on SoundCloud, ditto.
    pub soundcloud: Option<Track>,
}

impl CrossPlatformMatch {
    /// Anything to play besides the seed itself?
    pub fn has_other_provider(&self) -> bool {
        let seed = self.source.uri.provider();
        (seed != ProviderId::Spotify && self.spotify.is_some())
            || (seed != ProviderId::SoundCloud && self.soundcloud.is_some())
    }
}

pub(crate) async fn resolve_bridge(
    providers: &[Arc<dyn Provider>],
    source: Track,
) -> CrossPlatformMatch {
    let seed_provider = source.uri.provider();
    let q = Query {
        text: format!(
            "{} {}",
            source
                .artists
                .iter()
                .map(|a| a.name.clone())
                .next()
                .unwrap_or_default(),
            source.title,
        ),
        limit: Some(5),
    };
    let mut spotify = None;
    let mut soundcloud = None;

    let mut futs = FuturesUnordered::new();
    for p in providers {
        // Skip the seed's own provider — we already have that track.
        if p.id() == seed_provider {
            continue;
        }
        let p = p.clone();
        let q = q.clone();
        futs.push(async move { (p.id(), p.search(&q).await) });
    }
    while let Some((id, res)) = futs.next().await {
        if let Ok(results) = res
            && let Some(top) = results.tracks.into_iter().next()
        {
            match id {
                ProviderId::Spotify => spotify = Some(top),
                ProviderId::SoundCloud => soundcloud = Some(top),
                ProviderId::Local => {}
            }
        }
    }
    CrossPlatformMatch {
        source,
        spotify,
        soundcloud,
    }
}
```

### Step 4.2 — Wire into `DiscoveryEngine`

- [ ] In `discovery/src/lib.rs`, near the top (after the other `use` lines) add:

```rust
pub mod cross_platform;
pub use cross_platform::CrossPlatformMatch;
```

- [ ] At the bottom of the `impl DiscoveryEngine` block, add a new method (place it right after `similar_to` for readability):

```rust
    /// Find the same track on every *other* provider. Returns the seed
    /// untouched plus best-effort matches; the UI decides what to render
    /// based on `CrossPlatformMatch::has_other_provider`.
    pub async fn cross_platform_bridge(&self, source: Track) -> CrossPlatformMatch {
        cross_platform::resolve_bridge(&self.providers, source).await
    }
```

- [ ] Run: `command cargo check -p discovery`
- [ ] Expected: clean.

### Step 4.3 — Hook surface

- [ ] In `hooks/src/use_discovery.rs`, replace the entire file with:

```rust
//! Reactive surface for the discovery engine. Like `use_search`, this hook
//! is read-only — it produces a `results` list. Pages call
//! `queue.play_list(results, idx)` to actually play one.

use std::sync::Arc;

use dioxus::prelude::*;
use discovery::{
    CrossPlatformMatch, DiscoveryEngine, DiscoveryResult, SimilarToSeed,
};
use provider_api::Track;

/// Which mode the Discover page is currently driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    SimilarTo,
    CrossPlatformBridge,
}

#[derive(Clone)]
pub struct UseDiscovery {
    pub mode: Signal<DiscoveryMode>,
    pub input: Signal<String>,
    pub results: Signal<Vec<DiscoveryResult>>,
    pub bridge: Signal<Option<CrossPlatformMatch>>,
    pub is_searching: Signal<bool>,
    pub error: Signal<Option<String>>,
    engine: Arc<DiscoveryEngine>,
}

impl UseDiscovery {
    /// Kick off the active mode against the current input.
    pub fn run(&self) {
        match *self.mode.read() {
            DiscoveryMode::SimilarTo => self.run_similar(),
            DiscoveryMode::CrossPlatformBridge => self.run_bridge_from_input(),
        }
    }

    fn run_similar(&self) {
        let raw = self.input.read().clone();
        let seed = SimilarToSeed::from_input(&raw);
        if seed.title.is_empty() {
            self.error
                .clone()
                .set(Some("Type an artist and title first.".into()));
            return;
        }
        let engine = self.engine.clone();
        let mut results = self.results;
        let mut bridge = self.bridge;
        let mut is_searching = self.is_searching;
        let mut error = self.error;
        spawn(async move {
            is_searching.set(true);
            error.set(None);
            results.set(Vec::new());
            bridge.set(None);
            match engine.similar_to(seed).await {
                Ok(rs) => {
                    if rs.is_empty() {
                        error.set(Some(
                            "No neighbours found — try a more popular seed track.".into(),
                        ));
                    } else {
                        results.set(rs);
                    }
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            is_searching.set(false);
        });
    }

    /// CrossPlatformBridge entry point that operates on the typed input. The
    /// page also has a `bridge_from_track` path for when a Track is in hand.
    fn run_bridge_from_input(&self) {
        // We don't have a Track when only text is typed; the page disables
        // this path and uses `bridge_from_track` from result rows instead.
        // Keep the input-driven path as a no-op error so the button doesn't
        // silently do nothing.
        self.error
            .clone()
            .set(Some("Bridge mode plays from existing rows — pick a track first.".into()));
    }

    /// Bridge mode trigger that takes a concrete track. The Discover page
    /// wires this to row clicks when bridge mode is active.
    pub fn bridge_from_track(&self, source: Track) {
        let engine = self.engine.clone();
        let mut bridge = self.bridge;
        let mut results = self.results;
        let mut is_searching = self.is_searching;
        let mut error = self.error;
        spawn(async move {
            is_searching.set(true);
            error.set(None);
            results.set(Vec::new());
            let m = engine.cross_platform_bridge(source).await;
            if !m.has_other_provider() {
                error.set(Some(
                    "No match on another provider — try a more popular track.".into(),
                ));
                bridge.set(None);
            } else {
                bridge.set(Some(m));
            }
            is_searching.set(false);
        });
    }
}

pub fn use_discovery() -> UseDiscovery {
    let engine = use_context::<Arc<DiscoveryEngine>>();

    let mode = use_signal(|| DiscoveryMode::SimilarTo);
    let input = use_signal(String::new);
    let results = use_signal(Vec::<DiscoveryResult>::new);
    let bridge = use_signal(|| None::<CrossPlatformMatch>);
    let is_searching = use_signal(|| false);
    let error = use_signal(|| None::<String>);

    UseDiscovery {
        mode,
        input,
        results,
        bridge,
        is_searching,
        error,
        engine,
    }
}
```

- [ ] In `hooks/src/lib.rs`, re-export the new types. Find the line that re-exports from `use_discovery` (likely something like `pub use use_discovery::...`) and ensure it includes `CrossPlatformMatch, DiscoveryMode`. If unsure, add at the appropriate re-export block:

```rust
pub use discovery::{CrossPlatformMatch, DiscoveryResult};
pub use use_discovery::{DiscoveryMode, UseDiscovery, use_discovery};
```

(Adjust to match the file's existing style — only ensure these names are reachable.)

### Step 4.4 — Workspace check

- [ ] Run: `command cargo check --workspace`
- [ ] Expected: clean. If `hooks/src/lib.rs` re-export was wrong, fix and re-run.

### Step 4.5 — Discover page mode toggle

- [ ] In `pages/src/discover.rs`, update the imports at the top:

```rust
use dioxus::prelude::*;
use hooks::{
    CrossPlatformMatch, DiscoveryMode, DiscoveryResult, Track, use_discovery, use_queue,
};
```

- [ ] After the existing `let has_results = !results.is_empty();` line, add:

```rust
    let bridge_match = disc.bridge.read().clone();
    let current_mode = *disc.mode.read();
```

- [ ] Insert a mode toggle inside the `section.page` block, right after the `<h1>` and before the existing `p.hint`:

```rust
            div { class: "mode-toggle",
                button {
                    class: if current_mode == DiscoveryMode::SimilarTo {
                        "mode-btn active"
                    } else {
                        "mode-btn"
                    },
                    onclick: {
                        let disc = disc.clone();
                        move |_| disc.mode.clone().set(DiscoveryMode::SimilarTo)
                    },
                    "Similar to"
                }
                button {
                    class: if current_mode == DiscoveryMode::CrossPlatformBridge {
                        "mode-btn active"
                    } else {
                        "mode-btn"
                    },
                    onclick: {
                        let disc = disc.clone();
                        move |_| disc.mode.clone().set(DiscoveryMode::CrossPlatformBridge)
                    },
                    "Cross-platform bridge"
                }
            }
```

- [ ] At the end of the `section.page` block, before the closing brace, render the bridge result when present:

```rust
            if let Some(m) = bridge_match.as_ref() {
                BridgeResult {
                    bridge: m.clone(),
                    on_play: {
                        let queue = queue.clone();
                        let m = m.clone();
                        move |t: Track| {
                            queue.play_list(vec![t], 0);
                        }
                    }
                }
            }
```

- [ ] Add a new component at the bottom of the file:

```rust
#[component]
fn BridgeResult(bridge: CrossPlatformMatch, on_play: EventHandler<Track>) -> Element {
    let source = bridge.source.clone();
    let cover = source.cover_url.clone().unwrap_or_default();
    let source_label = source.uri.provider().label();
    let spotify = bridge.spotify.clone();
    let soundcloud = bridge.soundcloud.clone();

    rsx! {
        div { class: "bridge-result",
            div { class: "bridge-seed track-row",
                div { class: "track-cover",
                    if !cover.is_empty() {
                        img { src: "{cover}", alt: "", loading: "lazy" }
                    } else {
                        div { class: "track-cover-fallback",
                            i { class: "fa-solid fa-music" }
                        }
                    }
                }
                div { class: "track-meta",
                    div { class: "track-title", "{source.title}" }
                    div { class: "track-artist", "seed · {source_label}" }
                }
            }
            div { class: "bridge-matches",
                if let Some(sp) = spotify.as_ref() {
                    button {
                        class: "bridge-match-btn",
                        onclick: {
                            let sp = sp.clone();
                            move |_| on_play.call(sp.clone())
                        },
                        span { class: "track-badge spotify", "S" }
                        " Play on Spotify · {sp.title}"
                    }
                }
                if let Some(sc) = soundcloud.as_ref() {
                    button {
                        class: "bridge-match-btn",
                        onclick: {
                            let sc = sc.clone();
                            move |_| on_play.call(sc.clone())
                        },
                        span { class: "track-badge soundcloud", "SC" }
                        " Play on SoundCloud · {sc.title}"
                    }
                }
            }
        }
    }
}
```

- [ ] Update the `DiscoveryRow.on_play` closure so that in bridge mode, clicking a row triggers `bridge_from_track` instead of `play_list`. Inside the `for r in results.iter()` block in `Discover`, replace the existing `on_play:` closure with one that branches on `current_mode`:

```rust
                            on_play: {
                                let title = r.title.clone();
                                let artist = r.artist.clone();
                                let playable = playable.clone();
                                let queue = queue.clone();
                                let disc = disc.clone();
                                let result_for_bridge = r.clone();
                                move |_| {
                                    match current_mode {
                                        DiscoveryMode::SimilarTo => {
                                            if let Some(p_idx) = playable.iter().position(|t|
                                                t.title == title && t.artists.iter().any(|a| a.name == artist))
                                            {
                                                queue.play_list(playable.clone(), p_idx);
                                            }
                                        }
                                        DiscoveryMode::CrossPlatformBridge => {
                                            if let Some(t) = result_for_bridge.play_target() {
                                                disc.bridge_from_track(t);
                                            }
                                        }
                                    }
                                }
                            },
```

### Step 4.6 — Add the toggle CSS

- [ ] Append to `nira/assets/main.css`:

```css
.mode-toggle {
    display: flex;
    gap: 8px;
    margin: 8px 0 12px;
}

.mode-btn {
    background: transparent;
    border: 1px solid var(--border, #333);
    color: var(--fg-dim, #888);
    padding: 6px 12px;
    border-radius: 999px;
    cursor: pointer;
    font: inherit;
}

.mode-btn.active {
    background: var(--accent, #d4af37);
    border-color: var(--accent, #d4af37);
    color: #111;
}

.bridge-result {
    margin-top: 16px;
    padding: 12px;
    border: 1px solid var(--border, #333);
    border-radius: 8px;
}

.bridge-seed {
    padding-bottom: 8px;
    border-bottom: 1px dashed var(--border, #333);
    margin-bottom: 8px;
}

.bridge-matches {
    display: flex;
    flex-direction: column;
    gap: 6px;
}

.bridge-match-btn {
    background: transparent;
    border: 1px solid var(--border, #333);
    color: inherit;
    text-align: left;
    padding: 8px 12px;
    border-radius: 6px;
    cursor: pointer;
    font: inherit;
}

.bridge-match-btn:hover {
    background: rgba(255,255,255,0.04);
}
```

### Step 4.7 — Final check

- [ ] Run: `command cargo check --workspace`
- [ ] Expected: clean.
- [ ] Run: `command cargo build --workspace`
- [ ] Expected: build succeeds.

### Step 4.8 — Commit

```bash
git add discovery/src/cross_platform.rs discovery/src/lib.rs \
        hooks/src/use_discovery.rs hooks/src/lib.rs \
        pages/src/discover.rs nira/assets/main.css
git commit -m "feat(discovery): CrossPlatformBridge mode + UI toggle"
```

---

## Task 5: README + spec status note

**Files:**
- Modify: `docs/superpowers/specs/2026-05-16-nira-discovery-pivot-design.md`

### Step 5.1 — Mark Phase 3 complete

- [ ] In the spec's `## Phasing` section, replace the Phase 3 bullet:

```
- **Phase 3** — `discovery/` MVP with `SimilarTo` and `CrossPlatformBridge`; `enrichment/` with LB+MB+Last.fm clients and SQLite cache.
```

with:

```
- **Phase 3** — `discovery/` MVP with `SimilarTo` and `CrossPlatformBridge` ✓; `enrichment/` with LB+MB+Last.fm clients ✓ and disk-backed JSON cache (SQLite deferred — JSON cache satisfies TTL+keyed-lookup intent at our scale).
```

### Step 5.2 — Commit

```bash
git add docs/superpowers/specs/2026-05-16-nira-discovery-pivot-design.md
git commit -m "docs: mark Phase 3 (discovery + enrichment) complete"
```

---

## Verification

After all tasks:

- [ ] `command cargo check --workspace` clean
- [ ] `command cargo build --workspace` succeeds
- [ ] `command cargo test -p enrichment lastfm` 4 passed
- [ ] **Manual:** `dx serve --platform desktop` boots; Discover page shows mode toggle. With `NIRA_LASTFM_API_KEY` unset, SimilarTo still works (LB+SC). Bridge mode produces a result when clicking a row.
