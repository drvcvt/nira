//! In-process Spotify playback via librespot.
//!
//! Lifecycle:
//! 1. Caller hands us an OAuth access-token (managed by `provider-spotify`).
//! 2. We `Session::connect` once. The session lives for the rest of the
//!    process — librespot internally keeps the AP connection warm.
//! 3. `load_and_play(uri)` resolves the SpotifyUri and tells the player to
//!    start. librespot opens its own audio sink (rodio backend → cpal) the
//!    first time it needs to render audio.
//! 4. Playback state updates flow back via `PlayerEvent` on a tokio task,
//!    which mirrors them into our `SpotifyState` so `snapshot()` is cheap.
//!
//! Note: librespot uses its *own* cpal stream, separate from our rodio
//! mixer. Modern audio servers (PipeWire/PulseAudio) multiplex fine; nira's
//! coordinator stops the rodio sink whenever it routes to Spotify so they
//! don't double-play.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use librespot::core::Session;
use librespot::core::SessionConfig;
use librespot::core::authentication::Credentials;
use librespot::core::spotify_uri::SpotifyUri;
use librespot::playback::audio_backend;
use librespot::playback::config::{AudioFormat, PlayerConfig};
use librespot::playback::mixer::{self, Mixer, MixerConfig};
use librespot::playback::player::{Player as LibrespotPlayer, PlayerEvent};

use crate::equalizer::{EqualizedSink, EqualizerControl};

#[derive(Debug, thiserror::Error)]
pub enum SpotifyBackendError {
    #[error("librespot session connect: {0}")]
    Connect(String),
    #[error("librespot audio backend missing — feature flag mismatch")]
    NoBackend,
    #[error("librespot mixer missing — feature flag mismatch")]
    NoMixer,
    #[error("invalid spotify uri: {0}")]
    InvalidUri(String),
}

pub struct SpotifyBackend {
    player: Arc<LibrespotPlayer>,
    mixer: Arc<dyn Mixer>,
    state: Arc<RwLock<SpotifyState>>,
    /// Kept so `session_is_invalid` can detect when librespot shut the AP
    /// connection down (suspend/resume, network loss). A `Session` cannot
    /// reconnect once invalidated — callers drop the whole backend and build
    /// a fresh one.
    session: Session,
}

#[derive(Default, Clone)]
pub struct SpotifyState {
    pub is_paused: bool,
    pub has_track: bool,
    play_request_id: Option<u64>,
    /// Position reported by the most recent PlayerEvent. Combined with
    /// `last_position_at` it lets us extrapolate the live position between
    /// events.
    pub position_ms: u32,
    pub last_position_at: Option<Instant>,
}

impl SpotifyBackend {
    pub(crate) async fn new(
        access_token: &str,
        equalizer: EqualizerControl,
    ) -> Result<Self, SpotifyBackendError> {
        let session_config = SessionConfig::default();
        // Emit a position event every 500 ms while playing so our snapshot
        // tick can show a live timer in the bottombar. Normalisation on so
        // Spotify levels sit with the ReplayGain-normalised local files.
        let player_config = PlayerConfig {
            position_update_interval: Some(Duration::from_millis(500)),
            normalisation: true,
            ..PlayerConfig::default()
        };
        let audio_format = AudioFormat::default();

        let session = Session::new(session_config, None);
        let creds = Credentials::with_access_token(access_token);
        session
            .connect(creds, false)
            .await
            .map_err(|e| SpotifyBackendError::Connect(e.to_string()))?;
        tracing::info!("librespot session connected");

        let backend_fn = audio_backend::find(None).ok_or(SpotifyBackendError::NoBackend)?;
        let mixer_fn = mixer::find(None).ok_or(SpotifyBackendError::NoMixer)?;
        let mixer = mixer_fn(MixerConfig::default())
            .map_err(|e| SpotifyBackendError::Connect(format!("mixer open: {e}")))?;
        let volume_getter = mixer.get_soft_volume();

        let player =
            LibrespotPlayer::new(player_config, session.clone(), volume_getter, move || {
                Box::new(EqualizedSink::new(
                    backend_fn(None, audio_format),
                    equalizer,
                ))
            });

        let state = Arc::new(RwLock::new(SpotifyState::default()));
        spawn_event_listener(player.clone(), state.clone());

        Ok(Self {
            player,
            mixer,
            state,
            session,
        })
    }

    /// True once librespot has invalidated the session — it does that itself
    /// when the AP transport dies (suspend/resume, network drop). From that
    /// point every `load_and_play` is silently swallowed, so callers must
    /// treat the backend as dead and rebuild it.
    pub fn session_is_invalid(&self) -> bool {
        self.session.is_invalid()
    }

    pub fn load_and_play(&self, spotify_uri: &str) -> Result<(), SpotifyBackendError> {
        let track_id = SpotifyUri::from_uri(spotify_uri)
            .map_err(|e| SpotifyBackendError::InvalidUri(format!("{spotify_uri}: {e}")))?;
        self.player.load(track_id, true, 0);
        Ok(())
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn resume(&self) {
        self.player.play();
    }

    pub fn stop(&self) {
        self.player.stop();
        // Mirror the state synchronously instead of waiting for librespot's
        // `Stopped` event — the queue watcher polls `has_track` and a stale
        // `true` during a track switch reads as "the new load committed".
        if let Ok(mut s) = self.state.write() {
            s.has_track = false;
            s.is_paused = false;
            s.position_ms = 0;
            s.last_position_at = None;
        }
    }

    /// Jump to a millisecond offset inside the current track. Triggers a
    /// `Seeked` event back on the player channel, which the listener
    /// uses to refresh `position_ms` immediately so the UI doesn't tick
    /// backwards before the next periodic update lands.
    pub fn seek(&self, position_ms: u32) {
        self.player.seek(position_ms);
    }

    pub fn set_volume(&self, v: f32) {
        let vol_u16 = (v.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        self.mixer.set_volume(vol_u16);
    }

    /// Cheap snapshot built from the cached event state plus extrapolation.
    pub fn snapshot(&self) -> SpotifyState {
        // Writers hold the lock for a few field assignments — sync RwLock is
        // safe here, no risk of contention.
        let Ok(s_guard) = self.state.read() else {
            return SpotifyState::default();
        };
        let mut s = s_guard.clone();
        drop(s_guard);
        if !s.is_paused
            && s.has_track
            && let Some(t) = s.last_position_at
        {
            let elapsed = t.elapsed().as_millis() as u32;
            s.position_ms = s.position_ms.saturating_add(elapsed);
            // No `last_position_at` write here — `s` is a returned clone, so
            // this only ever looked like it advanced the anchor.
        }
        s
    }
}

fn spawn_event_listener(player: Arc<LibrespotPlayer>, state: Arc<RwLock<SpotifyState>>) {
    let mut events = player.get_player_event_channel();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let Ok(mut s) = state.write() else { continue };
            apply_event(&mut s, event);
        }
        tracing::info!("librespot event channel closed");
    });
}

fn apply_event(state: &mut SpotifyState, event: PlayerEvent) {
    if let PlayerEvent::PlayRequestIdChanged { play_request_id } = event {
        state.play_request_id = Some(play_request_id);
        return;
    }
    if event.get_play_request_id() != state.play_request_id {
        return;
    }

    match event {
        PlayerEvent::Playing { position_ms, .. } => {
            state.is_paused = false;
            state.has_track = true;
            state.position_ms = position_ms;
            state.last_position_at = Some(Instant::now());
        }
        PlayerEvent::Paused { position_ms, .. } => {
            state.is_paused = true;
            state.position_ms = position_ms;
            state.last_position_at = Some(Instant::now());
        }
        PlayerEvent::PositionChanged { position_ms, .. }
        | PlayerEvent::PositionCorrection { position_ms, .. }
        | PlayerEvent::Seeked { position_ms, .. } => {
            state.position_ms = position_ms;
            state.last_position_at = Some(Instant::now());
        }
        PlayerEvent::Stopped { .. }
        | PlayerEvent::EndOfTrack { .. }
        | PlayerEvent::Unavailable { .. } => {
            state.has_track = false;
            state.is_paused = false;
            state.position_ms = 0;
            state.last_position_at = None;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_stop_does_not_clear_current_track() {
        let track_id = SpotifyUri::from_uri("spotify:track:4uLU6hMCjMI75M1A2tKUQC").unwrap();
        let mut state = SpotifyState {
            has_track: true,
            ..SpotifyState::default()
        };

        apply_event(
            &mut state,
            PlayerEvent::PlayRequestIdChanged { play_request_id: 2 },
        );
        apply_event(
            &mut state,
            PlayerEvent::Stopped {
                play_request_id: 1,
                track_id,
            },
        );

        assert!(state.has_track);
    }
}
