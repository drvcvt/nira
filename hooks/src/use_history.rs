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

/// Refresh cadence. Lower than the player snapshot poll (100 ms) because the
/// log only grows on track-start, which is a human-scale event.
const REFRESH_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct UseHistory {
    pub entries: Signal<Vec<HistoryEntry>>,
}

pub fn use_history() -> UseHistory {
    let player = use_context::<Player>();
    let entries = use_signal(Vec::<HistoryEntry>::new);

    use_hook({
        let player = player.clone();
        move || {
            let mut entries_sig = entries;
            // Seed once on mount so the first frame has the persisted log.
            entries_sig.set(player.history().recent(RECENTLY_PLAYED_ROW));
            let player = player.clone();
            spawn(async move {
                loop {
                    tokio::time::sleep(REFRESH_EVERY).await;
                    let fresh = player.history().recent(RECENTLY_PLAYED_ROW);
                    // Set unconditionally — Signal equality is cheap and
                    // Vec equality dedup is built-in.
                    entries_sig.set(fresh);
                }
            });
        }
    });

    UseHistory { entries }
}
