//! Reactive surface for the audio engine.
//!
//! Pattern: the shell provisions a single `Player` + a single
//! `Signal<PlayerSnapshot>` into Dioxus context via [`PlayerContext::install`].
//! A background tick task polls the player's snapshot at an adaptive rate and
//! writes it into the signal — components subscribe to that signal and
//! re-render only when transport state actually changes.

use std::time::Duration;

use config::AppConfig;
use dioxus::prelude::*;
use player::{Player, PlayerSnapshot};

/// Provisioning helper called from the root `App` component. Sets up the
/// shared `Player` handle, the snapshot signal, and the polling future.
pub struct PlayerContext;

impl PlayerContext {
    pub fn install(player: Player) {
        let snapshot = use_signal({
            let player = player.clone();
            move || player.snapshot()
        });
        use_context_provider({
            let player = player.clone();
            move || player
        });
        use_context_provider(move || snapshot);

        // One global polling task — every consumer reads the same signal.
        //
        // Adaptive tempo: 200 ms while playback is active so the progress
        // bar stays smooth, 500 ms otherwise. Most of nira's lifetime is
        // spent idle, and at 100 ms unconditional we re-rendered the
        // bottombar 10×/s on a paused player for no UI delta.
        //
        // We also dedupe: PlayerSnapshot derives PartialEq, so we only
        // call snapshot.set() when the new value actually differs from
        // the previous tick. Dioxus signal updates trigger a re-render
        // unconditionally, so skipping no-op writes saves the entire
        // bottombar diff cost.
        use_hook({
            let player = player.clone();
            move || {
                let player = player;
                let mut snapshot = snapshot;
                spawn(async move {
                    let mut prev: Option<PlayerSnapshot> = None;
                    loop {
                        let snap = player.snapshot();
                        let active = snap.has_source && !snap.is_paused;
                        let changed = match &prev {
                            Some(p) => p != &snap,
                            None => true,
                        };
                        if changed {
                            snapshot.set(snap.clone());
                            prev = Some(snap);
                        }
                        let delay = if active { 200 } else { 500 };
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                });
            }
        });
    }
}

/// Public reactivity surface returned by [`use_player`].
#[derive(Clone)]
pub struct UsePlayer {
    player: Player,
    snapshot: Signal<PlayerSnapshot>,
    config: Signal<AppConfig>,
}

impl UsePlayer {
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.snapshot.read().clone()
    }

    /// Hand an in-memory audio buffer to the engine. Callers (typically the
    /// search page after downloading the SC stream URL) own the bytes and
    /// surrender them here.
    pub fn play_bytes(&self, bytes: Vec<u8>) -> Result<(), player::PlayerError> {
        self.player.play_bytes(bytes)
    }

    pub fn pause(&self) {
        self.player.pause();
    }
    pub fn resume(&self) {
        self.player.resume();
    }
    pub fn stop(&self) {
        self.player.stop();
    }
    pub fn seek(&self, target: std::time::Duration) {
        self.player.seek(target);
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        self.player.set_volume(v);

        let mut config = self.config;
        let to_save = {
            let mut cfg = config.write();
            if (cfg.volume - v).abs() < 0.005 {
                None
            } else {
                cfg.volume = v;
                Some(cfg.clone())
            }
        };
        if let Some(cfg) = to_save {
            if let Err(e) = cfg.save_bg() {
                tracing::warn!(error = %e, "volume persist failed");
            }
        }
    }

    pub fn clear_history(&self) -> std::io::Result<()> {
        self.player.clear_history()
    }

    /// Latest visualizer analysis frame (rodio backend only).
    pub fn viz_frame(&self) -> Option<player::VizFrame> {
        self.player.viz_frame()
    }

    /// Convenience for the transport-bar play button. Reads the *live*
    /// engine snapshot, not the polled signal — that one can lag by up to
    /// 500 ms, long enough to pause a paused player or treat a just-committed
    /// track as "nothing loaded". Returns `false` when no source is loaded
    /// so the caller can start the queue instead.
    pub fn toggle(&self) -> bool {
        let snap = self.player.snapshot();
        if !snap.has_source {
            return false;
        }
        if snap.is_paused {
            self.resume();
        } else {
            self.pause();
        }
        true
    }
}

pub fn use_player() -> UsePlayer {
    let player = use_context::<Player>();
    let snapshot = use_context::<Signal<PlayerSnapshot>>();
    let config = use_context::<Signal<AppConfig>>();
    UsePlayer {
        player,
        snapshot,
        config,
    }
}
