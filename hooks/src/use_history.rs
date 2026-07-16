//! Reactive surface for the local play-history log.
//!
//! The `History` struct in `player::history` owns the data; this hook
//! samples it into a Dioxus `Signal<Vec<HistoryEntry>>` so Home can subscribe
//! without polling the file directly. Refresh policy: re-read on a 30 s tick
//! while Home is mounted. Cheap — the in-memory `History` already buffers
//! everything; we're just diffing for the UI signal.

use std::time::Duration;

use dioxus::prelude::*;
use player::{HistoryEntry, Player};

/// How many rows the Home "Recently played" section shows.
const RECENTLY_PLAYED_ROW: usize = 8;

/// How many rows the recommendation engine pulls. Big enough that seed
/// selection can weight by recency and diversify by artist without being
/// stuck with the 8 entries the Recently-played strip displays.
const RECOMMENDATION_DEPTH: usize = 100;

/// Refresh cadence. Lower than the player snapshot poll (100 ms) because the
/// log only grows on track-start, which is a human-scale event.
const REFRESH_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct UseHistory {
    /// 8 newest rows, sized for the Home "Recently played" strip.
    pub entries: Signal<Vec<HistoryEntry>>,
    /// Up to 100 newest rows for the recommendation engine. Kept as a
    /// separate signal so consumers can't accidentally render 100 cover
    /// cards by reusing `entries`.
    pub deep_entries: Signal<Vec<HistoryEntry>>,
}

pub fn use_history() -> UseHistory {
    let player = use_context::<Player>();
    let entries = use_signal(Vec::<HistoryEntry>::new);
    let deep_entries = use_signal(Vec::<HistoryEntry>::new);

    use_hook({
        let player = player.clone();
        move || {
            let mut entries_sig = entries;
            let mut deep_sig = deep_entries;
            // Seed once on mount so the first frame has the persisted log.
            let deep = player.history().recent(RECOMMENDATION_DEPTH);
            entries_sig.set(deep.iter().take(RECENTLY_PLAYED_ROW).cloned().collect());
            deep_sig.set(deep);
            let player = player.clone();
            spawn(async move {
                // Dedupe against the last published value (same pattern as
                // the player-snapshot poll): Signal::set notifies subscribers
                // unconditionally, and an unchanged tick used to re-render
                // all of Home every 30 s — colliding with scroll/animations.
                let mut prev = deep_sig.peek().clone();
                loop {
                    tokio::time::sleep(REFRESH_EVERY).await;
                    let deep = player.history().recent(RECOMMENDATION_DEPTH);
                    if deep == prev {
                        continue;
                    }
                    prev = deep.clone();
                    let shallow: Vec<_> = deep.iter().take(RECENTLY_PLAYED_ROW).cloned().collect();
                    entries_sig.set(shallow);
                    deep_sig.set(deep);
                }
            });
        }
    });

    UseHistory {
        entries,
        deep_entries,
    }
}
