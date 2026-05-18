//! Reactive surface for the discovery engine. Like `use_search`, this hook
//! is read-only — it produces a `results` list. Pages call
//! `queue.play_list(results, idx)` to actually play one.
//!
//! Two modes share one input surface:
//! - `SimilarTo` (default) — typed seed → neighbourhood of related tracks.
//! - `CrossPlatformBridge` — picked from an existing row → the same track
//!   resolved on every other registered provider.

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
    /// Kick off the active mode against the current input. Bridge mode
    /// from text input is a no-op error message — bridge mode resolves
    /// from an existing track via `bridge_from_track`.
    pub fn run(&self) {
        match *self.mode.read() {
            DiscoveryMode::SimilarTo => self.run_similar(),
            DiscoveryMode::CrossPlatformBridge => {
                self.error.clone().set(Some(
                    "Bridge mode resolves a known track — type a seed in Similar-to mode, \
                     then click a result to bridge it."
                        .into(),
                ));
            }
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

    /// Whether the Last.fm path is wired (key present in config/env).
    /// Lets the UI distinguish `lf off` (no key) from `lf 0` (no hits).
    pub fn lastfm_configured(&self) -> bool {
        self.engine.lastfm_configured()
    }

    /// Bridge mode trigger that takes a concrete track. The Discover page
    /// wires this to row clicks when bridge mode is active.
    pub fn bridge_from_track(&self, source: Track) {
        let engine = self.engine.clone();
        let mut bridge = self.bridge;
        let mut is_searching = self.is_searching;
        let mut error = self.error;
        spawn(async move {
            is_searching.set(true);
            error.set(None);
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
