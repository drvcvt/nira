//! Reactive surface for the local play-history log.
//!
//! The `History` struct in `player::history` owns the data; this hook
//! samples it into Dioxus signals so Home can subscribe without polling the
//! file directly. Installed once from `AppContext::install` (context
//! singleton, same pattern as likes) so mutations like [`UseHistory::remove`]
//! update every subscriber immediately instead of waiting for the 30 s tick.

use std::time::Duration;

use dioxus::prelude::*;
use player::{HistoryEntry, Player};

/// How many rows the Home "Recently played" section shows.
const RECENTLY_PLAYED_ROW: usize = 8;

/// How many rows the recommendation engine pulls. Deep on purpose: the seed
/// sampler decays by age rather than cutting off, so older listens stay in
/// the pool as low-weight "rediscovery" candidates instead of vanishing.
const RECOMMENDATION_DEPTH: usize = 200;

/// Refresh cadence. Lower than the player snapshot poll (100 ms) because the
/// log only grows on track-start, which is a human-scale event.
const REFRESH_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct UseHistory {
    /// 8 newest rows, sized for the Home "Recently played" strip.
    pub entries: Signal<Vec<HistoryEntry>>,
    /// Up to 200 newest rows for the recommendation engine. Kept as a
    /// separate signal so consumers can't accidentally render 200 cover
    /// cards by reusing `entries`.
    pub deep_entries: Signal<Vec<HistoryEntry>>,
    player: Player,
}

impl UseHistory {
    /// Delete one entry from the log (history-card right-click → Remove)
    /// and refresh the signals immediately — no 30 s tick wait.
    pub fn remove(&self, entry: &HistoryEntry) {
        self.player.history().remove(entry.played_at, &entry.title);
        self.refresh();
    }

    fn refresh(&self) {
        let deep = self.player.history().recent(RECOMMENDATION_DEPTH);
        let mut entries = self.entries;
        let mut deep_sig = self.deep_entries;
        entries.set(deep.iter().take(RECENTLY_PLAYED_ROW).cloned().collect());
        deep_sig.set(deep);
    }
}

/// Install the singleton and spawn the 30 s refresher. Called once from
/// `AppContext::install`.
pub fn install_history(player: Player) {
    let entries = use_signal(Vec::<HistoryEntry>::new);
    let deep_entries = use_signal(Vec::<HistoryEntry>::new);
    let hist = UseHistory {
        entries,
        deep_entries,
        player,
    };
    use_context_provider({
        let hist = hist.clone();
        move || hist
    });

    use_hook(move || {
        // Seed once on install so the first frame has the persisted log.
        hist.refresh();
        let player = hist.player.clone();
        spawn(async move {
            let mut entries_sig = entries;
            let mut deep_sig = deep_entries;
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
    });
}

pub fn use_history() -> UseHistory {
    use_context::<UseHistory>()
}
