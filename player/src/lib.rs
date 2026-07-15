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
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::Utc;
use rodio::source::Source;
use rodio::stream::DeviceSinkBuilder;
use rodio::{Decoder, Player as RodioPlayer};
use stream_download::http::HttpStream;
use stream_download::http::reqwest::Client as HttpClient;
use stream_download::source::SourceStream;
use stream_download::storage::temp::TempStorageProvider;
use stream_download::{Settings as StreamSettings, StreamDownload};
use tokio::sync::mpsc as tokio_mpsc;

/// Progressive HTTP reader rodio decodes from while the download continues
/// in the background (temp-file backed, range-request seeks).
type HttpReader = StreamDownload<TempStorageProvider>;

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

/// What rodio is currently playing, kept so `seek` can rebuild the decoder.
/// Symphonia refuses backward seeks on some formats (notably MP3 from
/// in-memory bytes); when `try_seek` errors we re-decode from the retained
/// source and seek forward from zero, which every format supports.
#[derive(Clone)]
enum RodioSource {
    Bytes(Arc<[u8]>),
    File(PathBuf),
    /// Progressive stream — rebuild re-opens the (signed, still-valid) URL.
    Http {
        url: String,
    },
}

/// A progressive HTTP stream opened and decoder-wrapped, but not yet handed
/// to the audio engine. Splitting prepare (async, network) from play (sync,
/// engine state) lets the queue re-check that a load is still current after
/// the await, so a stale prepare can never clobber a newer track.
pub struct PreparedHttp {
    decoder: Decoder<HttpReader>,
    duration: Option<Duration>,
    url: String,
}

#[derive(Clone)]
pub struct Player {
    rodio: Arc<RodioPlayer>,
    /// Retained copy of rodio's current source for the seek fallback.
    rodio_source: Arc<RwLock<Option<RodioSource>>>,
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
            rodio_source: Arc::new(RwLock::new(None)),
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
        self.set_rodio_source(None);
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
        let bytes: Arc<[u8]> = bytes.into();
        let decoder = Self::decoder_from_bytes(&bytes)?;
        let dur = decoder.total_duration();
        self.set_rodio_source(Some(RodioSource::Bytes(bytes)));
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

    /// Play a local file via rodio (FLAC/MP3/M4A/OGG/WAV). Decodes straight
    /// from disk through a `BufReader<File>` instead of slurping the whole
    /// file into a `Vec` — a FLAC album track is tens of MB, and rodio's
    /// `TryFrom<File>` also records the byte length so seeking lands
    /// accurately. Stops Spotify first if it was active.
    ///
    /// `fallback_duration` is the tag-derived duration from the library scan;
    /// used when the decoder can't report one (e.g. VBR MP3 without a Xing
    /// header) so the seek bar stays usable.
    pub fn play_file(
        &self,
        path: &Path,
        fallback_duration: Option<Duration>,
    ) -> Result<(), PlayerError> {
        self.silence_spotify();
        let gain = Self::slider_to_gain(self.current_volume());
        let file = std::fs::File::open(path)
            .map_err(|e| PlayerError::Decode(format!("open {}: {e}", path.display())))?;
        let decoder = Decoder::try_from(file).map_err(|e| PlayerError::Decode(e.to_string()))?;
        let dur = decoder.total_duration().or(fallback_duration);
        self.set_rodio_source(Some(RodioSource::File(path.to_path_buf())));
        self.rodio.clear();
        // Re-assert log-curved gain before append — same first-5ms unity-gain
        // leak guard as play_bytes.
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

    /// Open a progressive HTTP stream: request the URL, prefetch ~256 KiB,
    /// wrap it in a decoder. Pure network/decode setup — no player state is
    /// touched, so callers can await this, re-check that the load is still
    /// wanted, then commit via [`Self::play_prepared`]. Playback starts as
    /// soon as the prefetch lands instead of after the whole file.
    pub async fn prepare_http(
        url: &str,
        fallback_duration: Option<Duration>,
    ) -> Result<PreparedHttp, PlayerError> {
        let (decoder, _len) = Self::http_decoder(url).await?;
        let duration = decoder.total_duration().or(fallback_duration);
        Ok(PreparedHttp {
            decoder,
            duration,
            url: url.to_string(),
        })
    }

    /// Hand a prepared progressive stream to the audio engine. Stops Spotify
    /// first, same as the other rodio entry points.
    pub fn play_prepared(&self, prepared: PreparedHttp) {
        self.silence_spotify();
        let PreparedHttp {
            decoder,
            duration,
            url,
        } = prepared;
        let gain = Self::slider_to_gain(self.current_volume());
        self.set_rodio_source(Some(RodioSource::Http { url }));
        self.rodio.clear();
        // Same first-5ms unity-gain leak guard as play_bytes.
        self.rodio.set_volume(gain);
        self.rodio.append(decoder);
        self.rodio.play();
        if let Ok(mut d) = self.duration.write() {
            *d = duration;
        }
        self.set_active(Active::Rodio);
        self.record_now_playing();
    }

    /// Progressive reader + decoder over an HTTP resource. Temp-file backed;
    /// explicit byte_len (from Content-Length) + seekable gives Symphonia
    /// full bidirectional seeking over the growing file.
    async fn http_decoder(url: &str) -> Result<(Decoder<HttpReader>, Option<u64>), PlayerError> {
        let parsed = url
            .parse()
            .map_err(|e| PlayerError::Decode(format!("bad stream url: {e}")))?;
        let stream = HttpStream::new(HttpClient::new(), parsed)
            .await
            .map_err(|e| PlayerError::Decode(format!("open stream: {e}")))?;
        let byte_len = stream.content_length();
        let reader = StreamDownload::from_stream(
            stream,
            TempStorageProvider::new(),
            StreamSettings::default(),
        )
        .await
        .map_err(|e| PlayerError::Decode(format!("start stream: {e}")))?;
        let mut builder = rodio::decoder::DecoderBuilder::new()
            .with_data(reader)
            .with_seekable(true);
        if let Some(len) = byte_len {
            builder = builder.with_byte_len(len);
        }
        builder
            .build()
            .map(|d| (d, byte_len))
            .map_err(|e| PlayerError::Decode(e.to_string()))
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
        self.stop_for_load();
        self.set_now_playing(None);
    }

    /// Stop audio output ahead of loading a new track. Leaves `now_playing`
    /// untouched — the incoming load overwrites it right away, and wiping it
    /// here would flash "Nothing loaded" between click and load-commit.
    /// Clearing the old source at load-begin is what keeps the queue watcher
    /// honest: the next `has_source=true` it sees can only belong to the
    /// newly committed track, never to the one being replaced.
    pub fn stop_for_load(&self) {
        self.silence_spotify();
        self.set_rodio_source(None);
        self.rodio.clear();
        if let Ok(mut d) = self.duration.write() {
            *d = None;
        }
        self.set_active(Active::None);
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
                    // Symphonia can't seek backward on some formats (MP3 from
                    // bytes in particular). Rebuild the decoder from the
                    // retained source and seek forward from zero instead.
                    tracing::info!("rodio seek failed ({e}); rebuilding decoder");
                    self.rodio_rebuild_seek(target);
                }
            }
            Active::None => {}
        }
    }

    /// Seek fallback: re-decode the retained source and forward-seek into the
    /// fresh decoder. Worst case (the forward seek fails too) the track
    /// restarts from zero — still better than a seek that silently no-ops.
    fn rodio_rebuild_seek(&self, target: Duration) {
        let source = self
            .rodio_source
            .read()
            .ok()
            .and_then(|s| s.as_ref().cloned());
        let Some(source) = source else { return };
        // Progressive streams rebuild asynchronously — respawn the stream
        // (the signed URL stays valid for the session), then forward-seek.
        if let RodioSource::Http { url } = &source {
            let Ok(handle) = tokio::runtime::Handle::try_current() else {
                tracing::warn!("no runtime available for http seek rebuild");
                return;
            };
            let player = self.clone();
            let url = url.clone();
            handle.spawn(async move {
                match Player::http_decoder(&url).await {
                    Ok((decoder, _)) => {
                        // The user may have switched tracks or stopped during
                        // the ~1 s rebuild — swapping in regardless would play
                        // the OLD track over whatever is current. Only commit
                        // if this URL is still the retained rodio source.
                        let still_current = player
                            .rodio_source
                            .read()
                            .ok()
                            .map(|s| {
                                matches!(s.as_ref(), Some(RodioSource::Http { url: u }) if *u == url)
                            })
                            .unwrap_or(false);
                        if still_current {
                            player.swap_in_and_seek(decoder, target);
                        } else {
                            tracing::info!("http seek rebuild superseded by a track change");
                        }
                    }
                    Err(e) => tracing::warn!("http seek rebuild failed: {e}"),
                }
            });
            return;
        }
        let outcome = match &source {
            RodioSource::Bytes(bytes) => {
                Self::decoder_from_bytes(bytes).map(|d| self.swap_in_and_seek(d, target))
            }
            RodioSource::File(path) => std::fs::File::open(path)
                .map_err(|e| PlayerError::Decode(format!("reopen {}: {e}", path.display())))
                .and_then(|f| {
                    Decoder::try_from(f).map_err(|e| PlayerError::Decode(e.to_string()))
                })
                .map(|d| self.swap_in_and_seek(d, target)),
            RodioSource::Http { .. } => return, // handled above (async path)
        };
        if let Err(e) = outcome {
            tracing::warn!("seek rebuild failed, keeping current position: {e}");
        }
    }

    fn swap_in_and_seek<S>(&self, decoder: S, target: Duration)
    where
        S: Source + Send + 'static,
    {
        let was_paused = self.rodio.is_paused();
        let gain = Self::slider_to_gain(self.current_volume());
        self.rodio.clear();
        // Same first-5ms unity-gain leak guard as play_bytes.
        self.rodio.set_volume(gain);
        self.rodio.append(decoder);
        if let Err(e) = self.rodio.try_seek(target) {
            tracing::warn!("forward seek into fresh decoder failed: {e}");
        }
        if !was_paused {
            self.rodio.play();
        }
    }

    /// Decoder over shared bytes; explicit byte_len + seekable so Symphonia
    /// gets the best seeking setup the source allows.
    fn decoder_from_bytes(bytes: &Arc<[u8]>) -> Result<Decoder<Cursor<Arc<[u8]>>>, PlayerError> {
        rodio::decoder::DecoderBuilder::new()
            .with_data(Cursor::new(Arc::clone(bytes)))
            .with_byte_len(bytes.len() as u64)
            .with_seekable(true)
            .build()
            .map_err(|e| PlayerError::Decode(e.to_string()))
    }

    fn set_rodio_source(&self, source: Option<RodioSource>) {
        if let Ok(mut w) = self.rodio_source.write() {
            *w = source;
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
