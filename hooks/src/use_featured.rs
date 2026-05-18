//! "Try this" hero card on Home.
//!
//! Picks a random Liked Song as a seed, runs `DiscoveryEngine::similar_to`
//! on it, and surfaces the top neighbour as the featured suggestion. The
//! seed and the result are exposed as separate signals so the card can show
//! the *why* ("based on your love of …") alongside the *what*.
//!
//! Library state arrives via the caller — Home already mounts `use_library`
//! to drive its other rows, and we want to share that snapshot rather than
//! spawn a second background refresh.

use std::sync::Arc;

use dioxus::prelude::*;
use discovery::{DiscoveryEngine, DiscoveryResult, SimilarToSeed};
use provider_api::Track;
use rand::Rng;

use crate::UseLibrary;

#[derive(Clone)]
pub struct UseFeatured {
    pub suggestion: Signal<Option<DiscoveryResult>>,
    /// The Liked-Songs track currently driving the recommendation. Shown
    /// under the card so users see *why* this track surfaced.
    pub seed: Signal<Option<Track>>,
    pub is_loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// `true` when the user's liked library is empty (or Spotify isn't
    /// connected yet). The card renders a friendlier CTA in that state
    /// instead of an error banner.
    pub needs_library: Signal<bool>,
    engine: Arc<DiscoveryEngine>,
    library_liked: Signal<Vec<Track>>,
}

// Manual `PartialEq` so this handle can be a component prop. Arc fields
// don't implement `PartialEq` themselves, but for our purposes they're
// boot-time constants — equality is fully determined by the signal set.
impl PartialEq for UseFeatured {
    fn eq(&self, other: &Self) -> bool {
        self.suggestion == other.suggestion
            && self.seed == other.seed
            && self.is_loading == other.is_loading
            && self.error == other.error
            && self.needs_library == other.needs_library
    }
}

impl UseFeatured {
    /// Pick a fresh random Liked Song and run discovery on it. Safe to call
    /// repeatedly — overlapping calls will both run, but the latest one's
    /// result wins on signal assignment (later spawn closes after the
    /// earlier one when the network round-trip is shorter, but the UI
    /// reads only the most recent `suggestion`).
    pub fn reroll(&self) {
        let liked = self.library_liked.read().clone();
        let mut needs = self.needs_library;
        if liked.is_empty() {
            needs.set(true);
            return;
        }
        needs.set(false);

        let idx = rand::rng().random_range(0..liked.len());
        let track = liked[idx].clone();
        let seed_input = SimilarToSeed {
            artist: track
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            title: track.title.clone(),
            mbid: track.mbid.clone(),
        };

        let mut current_seed = self.seed;
        let mut suggestion = self.suggestion;
        let mut is_loading = self.is_loading;
        let mut error = self.error;
        let engine = self.engine.clone();

        current_seed.set(Some(track));
        spawn(async move {
            is_loading.set(true);
            error.set(None);
            suggestion.set(None);
            match engine.similar_to(seed_input).await {
                Ok(rs) => {
                    if let Some(top) = rs.into_iter().next() {
                        suggestion.set(Some(top));
                    } else {
                        error.set(Some(
                            "No neighbours found for that seed — give it another roll.".into(),
                        ));
                    }
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            is_loading.set(false);
        });
    }
}

/// Build a featured-suggestion hook. Takes the caller's `UseLibrary` so it
/// reads the same `liked` signal Home is already watching, instead of
/// spawning a second Spotify refresh task.
pub fn use_featured(library: UseLibrary) -> UseFeatured {
    let engine = use_context::<Arc<DiscoveryEngine>>();
    let suggestion = use_signal(|| None::<DiscoveryResult>);
    let seed = use_signal(|| None::<Track>);
    let is_loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let needs_library = use_signal(|| false);

    let handle = UseFeatured {
        suggestion,
        seed,
        is_loading,
        error,
        needs_library,
        engine,
        library_liked: library.liked,
    };

    // First-roll trigger: when the liked library transitions from empty
    // to non-empty (or is already non-empty on first mount), kick off a
    // single discovery run. Subsequent re-rolls go through `reroll()` via
    // the Refresh button.
    {
        let handle = handle.clone();
        use_effect(move || {
            let liked_len = library.liked.read().len();
            let has_suggestion = handle.suggestion.peek().is_some();
            let in_flight = *handle.is_loading.peek();
            let has_error = handle.error.peek().is_some();
            if liked_len == 0 {
                // Surface the CTA without thrashing the signal.
                let mut needs = handle.needs_library;
                if !*needs.peek() {
                    needs.set(true);
                }
                return;
            }
            if has_suggestion || in_flight || has_error {
                return;
            }
            handle.reroll();
        });
    }

    handle
}
