//! Background scrobble watcher.
//!
//! Polls the player snapshot every 2 s. When the track changes, sends
//! `playing_now` to ListenBrainz. When the track has been actively playing
//! for `min(50%, 4min)`, sends a `single` listen with the wallclock start
//! time. Disabled (no-op poll loop) until the user pastes a token in
//! Settings — we read that out of `use_config` on each tick so the change
//! takes effect without an app restart.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use config::AppConfig;
use dioxus::prelude::*;
use enrichment::EnrichmentClient;
use player::Player;

/// Install the scrobble watcher into the current Dioxus scope. Called once
/// from `AppContext::install`.
pub fn install(player: Player, enrichment: Arc<EnrichmentClient>, config: Signal<AppConfig>) {
    use_hook(move || {
        let player = player.clone();
        let enrichment = enrichment.clone();
        let config = config;
        spawn(async move {
            let mut watcher = ScrobbleWatcher::default();
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;

                let token = config.peek().listenbrainz_token.clone();
                let Some(token) = token.filter(|t| !t.trim().is_empty()) else {
                    // No token — reset state so a later configure starts fresh.
                    watcher = ScrobbleWatcher::default();
                    continue;
                };
                let snap = player.snapshot();
                let Some(np) = snap.now_playing.as_ref() else {
                    watcher.current_playback_id = None;
                    continue;
                };
                if !snap.has_source {
                    continue;
                }

                // New track? Reset per-track state + ping playing_now.
                if watcher.start_if_new(snap.playback_id) {
                    watcher.started_at_unix = now_unix();
                    watcher.scrobbled = false;
                    let enrichment = enrichment.clone();
                    let token = token.clone();
                    let title = np.title.clone();
                    let artist = np.artist.clone();
                    tokio::spawn(async move {
                        if let Err(e) = enrichment.lb_playing_now(&token, &title, &artist).await {
                            tracing::warn!(error = %e, "playing_now failed");
                        }
                    });
                    continue;
                }

                if watcher.scrobbled || snap.is_paused {
                    continue;
                }

                // Threshold: min(50% of duration, 4 min). If duration unknown
                // (streaming source without LAME header), use 4 min.
                let half = snap
                    .duration
                    .map(|d| d / 2)
                    .unwrap_or_else(|| Duration::from_secs(240));
                let four_min = Duration::from_secs(240);
                let threshold = std::cmp::min(half, four_min);
                if snap.position >= threshold {
                    watcher.scrobbled = true;
                    let enrichment = enrichment.clone();
                    let token = token.clone();
                    let title = np.title.clone();
                    let artist = np.artist.clone();
                    let started = watcher.started_at_unix;
                    tokio::spawn(async move {
                        if let Err(e) = enrichment
                            .lb_submit_listen(&token, &title, &artist, started)
                            .await
                        {
                            tracing::warn!(error = %e, "submit-listens failed");
                        } else {
                            tracing::info!(track = %title, artist = %artist, "scrobbled");
                        }
                    });
                }
            }
        });
    });
}

#[derive(Default)]
struct ScrobbleWatcher {
    /// Player-assigned playback identity. Unlike artist/title or URI, this
    /// changes when the same track repeats.
    current_playback_id: Option<u64>,
    started_at_unix: u64,
    scrobbled: bool,
}

impl ScrobbleWatcher {
    fn start_if_new(&mut self, playback_id: u64) -> bool {
        if self.current_playback_id == Some(playback_id) {
            return false;
        }
        self.current_playback_id = Some(playback_id);
        true
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::ScrobbleWatcher;

    #[test]
    fn repeated_track_with_new_playback_id_resets_watcher() {
        let mut watcher = ScrobbleWatcher::default();
        assert!(watcher.start_if_new(10));
        assert!(!watcher.start_if_new(10));
        assert!(watcher.start_if_new(11));
    }
}
