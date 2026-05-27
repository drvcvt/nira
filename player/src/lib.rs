//! Audio engine — coordinates two backends:
//!
//! - **rodio** for in-memory bytes (SoundCloud progressive streams, future
//!   local files, test tone). Owns its own `cpal::Stream` via a parked
//!   worker thread so the `!Send` parts of cpal don't infect the public
//!   Player handle.
//! - **librespot** for Spotify tracks (lazy-initialised on first SP play
//!   call). librespot has its own cpal stream + audio task; we treat it as
//!   a separate engine and route commands accordingly.
//!
//! At any moment exactly one backend is "active" — the other is idle. The
//! `Active` flag drives pause/resume/stop/volume routing and snapshot
//! shape. UI consumes `PlayerSnapshot` and never knows which backend
//! produced it.

mod history;
mod spotify_backend;

pub use history::{History, HistoryEntry};
pub use spotify_backend::{SpotifyBackend, SpotifyBackendError};

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::Utc;
use rodio::source::Source;
use rodio::stream::DeviceSinkBuilder;
use rodio::{Decoder, Player as RodioPlayer};
use tokio::sync::mpsc as tokio_mpsc;

/// Out-of-band transport requests routed through the audio engine.
///
/// Used when something outside the Dioxus render tree (MPRIS, system media
/// keys, future global hotkeys) needs to drive next/previous on the queue.
/// Sent via `Player::request_next/request_prev`, received once by the queue
/// install path via `Player::take_transport_rx`.
#[derive(Debug, Clone, Copy)]
pub enum TransportCmd {
    Next,
    Previous,
    Stop,
}

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("failed to open default audio device: {0}")]
    Device(String),
    #[error("failed to decode audio: {0}")]
    Decode(String),
    #[error("audio worker thread died before init")]
    WorkerDied,
    #[error("spotify backend: {0}")]
    Spotify(#[from] SpotifyBackendError),
    #[error("spotify backend not initialised — connect via Settings first")]
    SpotifyNotReady,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub cover_url: Option<String>,
    pub source_label: String,
    pub provider: String,
    /// Provider-scoped URI string, when playback came from a real track.
    /// Used by History/Home to replay exact entries instead of text-searching.
    pub track_uri: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Active {
    None,
    Rodio,
    Spotify,
}

#[derive(Clone)]
pub struct Player {
    rodio: Arc<RodioPlayer>,
    spotify: Arc<Mutex<Option<Arc<SpotifyBackend>>>>,
    active: Arc<RwLock<Active>>,
    duration: Arc<RwLock<Option<Duration>>>,
    now_playing: Arc<RwLock<Option<NowPlaying>>>,
    history: History,
    /// Canonical 0.0..1.0 volume. The bottombar slider reads/writes this; we
    /// also apply it statically as `.amplify(v)` on each source we hand to
    /// rodio because rodio 0.22's dynamic `controls.volume` had a window at
    /// track-start where the first ~5 ms of audio leaked through at unity
    /// gain — perceived as full-volume earrape when a track changes.
    user_volume: Arc<RwLock<f32>>,
    /// Outbound side of the transport bus. Sends `Next`/`Previous` from
    /// MPRIS (and future media-key surfaces) to whatever consumer the queue
    /// install path wired up. Unbounded so an off-thread MPRIS request never
    /// blocks the audio code path.
    transport_tx: tokio_mpsc::UnboundedSender<TransportCmd>,
    /// Inbound side, taken exactly once by the queue install path. Wrapped
    /// in a Mutex/Option so subsequent calls return None — guards against
    /// accidental double-install.
    transport_rx: Arc<Mutex<Option<tokio_mpsc::UnboundedReceiver<TransportCmd>>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSnapshot {
    pub is_paused: bool,
    pub volume: f32,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub has_source: bool,
    pub now_playing: Option<NowPlaying>,
    /// Which backend produced this snapshot. Used by hooks for routing
    /// decisions and by the bottombar for the source dot colour.
    pub active: Active,
}

impl Player {
    /// Boot the audio worker. `history_path`, when present, points at the
    /// JSONL play-log Home consumes — passed in (rather than resolved here)
    /// so `player/` doesn't need to know about `config/`.
    pub fn spawn(history_path: Option<PathBuf>, initial_volume: f32) -> Result<Self, PlayerError> {
        let (tx, rx) = mpsc::sync_channel::<Result<Arc<RodioPlayer>, PlayerError>>(1);

        std::thread::Builder::new()
            .name("nira-audio".into())
            .spawn(move || {
                let device_sink = match DeviceSinkBuilder::open_default_sink() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(Err(PlayerError::Device(e.to_string())));
                        return;
                    }
                };
                let player = Arc::new(RodioPlayer::connect_new(device_sink.mixer()));
                let _ = tx.send(Ok(Arc::clone(&player)));
                let _keep = (device_sink, player);
                loop {
                    std::thread::park();
                }
            })
            .map_err(|e| PlayerError::Device(format!("spawn worker: {e}")))?;

        let rodio = rx.recv().map_err(|_| PlayerError::WorkerDied)??;
        let initial_volume = initial_volume.clamp(0.0, 1.0);
        // Configured slider position → log-curved gain so the initial level
        // matches what the Spotify side will hand back too.
        rodio.set_volume(Self::slider_to_gain(initial_volume));
        let (transport_tx, transport_rx) = tokio_mpsc::unbounded_channel();
        Ok(Player {
            rodio,
            spotify: Arc::new(Mutex::new(None)),
            active: Arc::new(RwLock::new(Active::None)),
            duration: Arc::new(RwLock::new(None)),
            now_playing: Arc::new(RwLock::new(None)),
            history: History::open(history_path),
            user_volume: Arc::new(RwLock::new(initial_volume)),
            transport_tx,
            transport_rx: Arc::new(Mutex::new(Some(transport_rx))),
        })
    }

    /// Send a transport command from off-thread surfaces (MPRIS, future
    /// system-media-key bindings). The receiver is owned by the queue; if
    /// no consumer is installed yet, the command is dropped silently —
    /// best-effort, losing a media-key press is preferable to blocking the
    /// audio thread.
    pub fn request_next(&self) {
        let _ = self.transport_tx.send(TransportCmd::Next);
    }

    pub fn request_previous(&self) {
        let _ = self.transport_tx.send(TransportCmd::Previous);
    }

    pub fn request_stop(&self) {
        let _ = self.transport_tx.send(TransportCmd::Stop);
    }

    /// Take the single receiver. Returns `Some` exactly once per Player —
    /// subsequent calls yield `None`. Queue install owns this on app boot.
    pub fn take_transport_rx(&self) -> Option<tokio_mpsc::UnboundedReceiver<TransportCmd>> {
        self.transport_rx.lock().ok().and_then(|mut g| g.take())
    }

    fn current_volume(&self) -> f32 {
        *self.user_volume.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Map a 0..1 slider position to a linear gain via a 60 dB log curve —
    /// the same curve librespot uses internally for its `SoftMixer`. Without
    /// this, rodio (which scales linearly) feels ~ten times louder than
    /// Spotify at the same slider position. 60 dB matches the desktop-audio
    /// convention (and is the librespot default).
    fn slider_to_gain(v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.0 {
            0.0
        } else {
            // 10 ^ ((v - 1) * 60 / 20) == 10 ^ ((v - 1) * 3)
            10f32.powf((v - 1.0) * 3.0)
        }
    }

    /// Read-only handle to the play log so hooks can subscribe to it.
    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn clear_history(&self) -> std::io::Result<()> {
        self.history.clear()
    }

    pub fn set_now_playing(&self, np: Option<NowPlaying>) {
        if let Ok(mut w) = self.now_playing.write() {
            *w = np;
        }
    }

    /// Test tone via rodio. Stops Spotify first.
    pub fn play_test_tone(&self) {
        self.silence_spotify();
        self.rodio.clear();
        // 0.2 headroom keeps the sine from clipping even at user-volume 1.0.
        // rodio's controls.volume layers on top.
        let tone = rodio::source::SineWave::new(440.0)
            .take_duration(Duration::from_secs(30))
            .amplify(0.2);
        self.rodio
            .set_volume(Self::slider_to_gain(self.current_volume()));
        self.rodio.append(tone);
        self.rodio.play();
        if let Ok(mut d) = self.duration.write() {
            *d = Some(Duration::from_secs(30));
        }
        self.set_active(Active::Rodio);
        self.set_now_playing(Some(NowPlaying {
            title: "Test tone".into(),
            artist: "440 Hz".into(),
            cover_url: None,
            source_label: "test signal".into(),
            provider: "Local".into(),
            track_uri: None,
        }));
    }

    /// Play decoded bytes via rodio (SoundCloud progressive, future locals).
    /// Stops Spotify if it was active so we don't double-play.
    pub fn play_bytes(&self, bytes: Vec<u8>) -> Result<(), PlayerError> {
        self.silence_spotify();
        let gain = Self::slider_to_gain(self.current_volume());
        let cursor = Cursor::new(bytes);
        let decoder = Decoder::try_from(cursor).map_err(|e| PlayerError::Decode(e.to_string()))?;
        let dur = decoder.total_duration();
        self.rodio.clear();
        // Re-assert log-curved gain *before* append so the very first 5 ms
        // of the new source can't leak through at unity gain — rodio's
        // periodic_access tick runs on the first poll, which happens at
        // append time.
        self.rodio.set_volume(gain);
        self.rodio.append(decoder);
        self.rodio.play();
        if let Ok(mut d) = self.duration.write() {
            *d = dur;
        }
        self.set_active(Active::Rodio);
        self.record_now_playing();
        Ok(())
    }

    /// Ensure a Spotify session exists. Idempotent — once a session is up we
    /// reuse it; if it has dropped (no field exposes that today) callers can
    /// `reset_spotify()` first to force a fresh connect on the next call.
    pub async fn ensure_spotify(&self, access_token: &str) -> Result<(), PlayerError> {
        // Fast path: already initialised.
        if matches!(self.spotify.lock(), Ok(g) if g.is_some()) {
            return Ok(());
        }
        // Slow path: connect outside the mutex.
        let backend = SpotifyBackend::new(access_token).await?;
        // Apply nira's current volume *before* the user can hear the first
        // packet — librespot defaults to 100% which is earrape if the
        // bottombar was sitting at 5%.
        backend.set_volume(self.current_volume());

        let mut guard = self.spotify.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(Arc::new(backend));
        }
        Ok(())
    }

    /// Drop the Spotify backend, e.g. after the user disconnects. The next
    /// `ensure_spotify` will fully reconnect.
    pub fn reset_spotify(&self) {
        if let Some(b) = self
            .spotify
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            b.stop();
            drop(b);
        }
        // If Spotify was the active backend, mark idle.
        let mut a = self.active.write().unwrap_or_else(|p| p.into_inner());
        if matches!(*a, Active::Spotify) {
            *a = Active::None;
            if let Ok(mut d) = self.duration.write() {
                *d = None;
            }
        }
    }

    /// Hand a spotify:track:… URI to librespot. Requires `ensure_spotify`
    /// has already succeeded.
    pub fn play_spotify(&self, uri: &str, duration: Option<Duration>) -> Result<(), PlayerError> {
        let backend = self
            .spotify
            .lock()
            .unwrap()
            .clone()
            .ok_or(PlayerError::SpotifyNotReady)?;
        self.rodio.clear();
        backend.load_and_play(uri)?;
        if let Ok(mut d) = self.duration.write() {
            *d = duration;
        }
        self.set_active(Active::Spotify);
        self.record_now_playing();
        Ok(())
    }

    pub fn pause(&self) {
        match *self.active.read().unwrap_or_else(|p| p.into_inner()) {
            Active::Spotify => {
                if let Some(b) = self
                    .spotify
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                {
                    b.pause();
                }
            }
            _ => self.rodio.pause(),
        }
    }

    pub fn resume(&self) {
        match *self.active.read().unwrap_or_else(|p| p.into_inner()) {
            Active::Spotify => {
                if let Some(b) = self
                    .spotify
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                {
                    b.resume();
                }
            }
            _ => self.rodio.play(),
        }
    }

    pub fn stop(&self) {
        self.silence_spotify();
        self.rodio.clear();
        if let Ok(mut d) = self.duration.write() {
            *d = None;
        }
        self.set_active(Active::None);
        self.set_now_playing(None);
    }

    /// Jump to `target` inside the currently playing source.
    /// Best-effort — rodio can fail to seek on decoders that don't
    /// support it (we log and move on); librespot accepts a ms target
    /// and dispatches a `Seeked` event when done.
    pub fn seek(&self, target: Duration) {
        match *self.active.read().unwrap_or_else(|p| p.into_inner()) {
            Active::Spotify => {
                if let Some(b) = self
                    .spotify
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                {
                    let ms = u32::try_from(target.as_millis()).unwrap_or(u32::MAX);
                    b.seek(ms);
                }
            }
            Active::Rodio => {
                if let Err(e) = self.rodio.try_seek(target) {
                    tracing::warn!("rodio seek failed: {e}");
                }
            }
            Active::None => {}
        }
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        if let Ok(mut w) = self.user_volume.write() {
            *w = v;
        }
        // Convert the perceptual slider position to a linear gain via the
        // 60 dB log curve. librespot's `SoftMixer` already applies the same
        // curve internally, so we pass the raw slider value over there.
        self.rodio.set_volume(Self::slider_to_gain(v));
        if let Some(b) = self
            .spotify
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            b.set_volume(v);
        }
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        let active = *self.active.read().unwrap_or_else(|p| p.into_inner());
        let (is_paused, position, has_source) = match active {
            Active::Spotify => match self
                .spotify
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
            {
                Some(b) => {
                    let s = b.snapshot();
                    (
                        s.is_paused,
                        Duration::from_millis(s.position_ms as u64),
                        s.has_track,
                    )
                }
                None => (false, Duration::ZERO, false),
            },
            _ => (
                self.rodio.is_paused(),
                self.rodio.get_pos(),
                !self.rodio.empty(),
            ),
        };
        // Read our canonical user_volume (kept in sync with rodio + librespot
        // by set_volume) so the slider always reflects the user's intent
        // even between backend switches.
        let volume = self.current_volume();
        PlayerSnapshot {
            is_paused,
            volume,
            position,
            duration: self.duration.read().ok().and_then(|d| *d),
            has_source,
            now_playing: self.now_playing.read().ok().and_then(|n| n.clone()),
            active,
        }
    }

    fn silence_spotify(&self) {
        if let Some(b) = self
            .spotify
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            && *self.active.read().unwrap_or_else(|p| p.into_inner()) == Active::Spotify
        {
            b.stop();
        }
    }

    fn set_active(&self, a: Active) {
        if let Ok(mut w) = self.active.write() {
            *w = a;
        }
    }

    /// Pull the current `now_playing` and append it to the local history log,
    /// if any is set. Best-effort — never panics, never propagates errors,
    /// because "couldn't write Home's log" must not break audio.
    fn record_now_playing(&self) {
        let snapshot = self.now_playing.read().ok().and_then(|n| n.clone());
        let Some(np) = snapshot else {
            return;
        };
        // Skip the synthesised test tone — it's debug output, not music.
        if np.provider == "Local" && np.source_label == "test signal" {
            return;
        }
        self.history.record(HistoryEntry {
            title: np.title,
            artist: np.artist,
            provider: np.provider,
            track_uri: np.track_uri,
            cover_url: np.cover_url,
            played_at: Utc::now(),
        });
    }
}
