//! Playback queue + auto-advance.
//!
//! The queue is the user-facing notion of "what plays next." Pages call
//! `play_list(tracks, idx)` to seed it from whatever list they're showing
//! (search results, library, discovery candidates). Once seeded:
//!
//! - Manual transport via `next() / previous() / stop()` walks the index.
//! - A background watcher polls `Player::snapshot()` every 500 ms and, when
//!   it sees the active backend's `has_source` go from `true` to `false`
//!   *while we're in the `Playing` state*, treats that as a natural track
//!   end and advances to the next entry.
//! - User-initiated stop sets the state to `Idle` so the same `has_source`
//!   transition doesn't get mistaken for an end-of-track.
//!
//! The queue *owns* the now-playing/loading/error state for track playback;
//! per-page hooks just read out of it. Search-side errors (network failures
//! during query) stay in the page-specific hooks.

use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;
use discovery::{DiscoveryEngine, SimilarToSeed};
use player::{NowPlaying, Player, TransportCmd};
use provider_api::{Provider, ProviderId, StreamHandle, Track};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;
use rand::seq::SliceRandom;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AdvanceState {
    /// Queue is empty or the user stopped. Watcher takes no action.
    Idle,
    /// `load_current` has been issued; we're waiting for the backend to
    /// surface `has_source=true`.
    Loading,
    /// Backend is rendering audio. A transition to `has_source=false` here
    /// means the track ended naturally.
    Playing,
}

#[derive(Clone)]
pub struct UseQueue {
    pub entries: Signal<Vec<Track>>,
    pub current_index: Signal<Option<usize>>,
    pub shuffle_enabled: Signal<bool>,
    pub repeat_mode: Signal<RepeatMode>,
    /// Track-load error (decode failed, no playable provider, etc.). Cleared
    /// on every fresh `play_list` / `next` / `previous`.
    pub error: Signal<Option<String>>,
    /// True while the currently-selected track is being fetched + handed to
    /// the audio engine. Goes false once playback starts (or fails).
    pub is_loading_track: Signal<bool>,
    advance_state: Signal<AdvanceState>,
    load_generation: Signal<u64>,
    pub radio_status: Signal<RadioStatus>,
    sc: Arc<SoundCloudProvider>,
    sp: Arc<SpotifyProvider>,
    player: Player,
}

/// Lifecycle of a Song Radio fetch. Surfaced via `UseQueue::radio_status`
/// so UI can show a toast / spinner while we wait for the discovery
/// engine to come back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RadioStatus {
    Idle,
    Loading,
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl UseQueue {
    /// Reset the queue to `tracks` and start playing `start_idx`. `start_idx`
    /// is clamped into bounds; a click on entry 17 of a 50-entry list ends
    /// up at index 17. No-op if `tracks` is empty.
    pub fn play_list(&self, tracks: Vec<Track>, start_idx: usize) {
        if tracks.is_empty() {
            return;
        }
        let idx = start_idx.min(tracks.len() - 1);
        let (tracks, idx) = if *self.shuffle_enabled.peek() {
            shuffled_context(tracks, idx)
        } else {
            (tracks, idx)
        };
        let mut entries = self.entries;
        let mut current = self.current_index;
        let mut state = self.advance_state;
        entries.set(tracks);
        current.set(Some(idx));
        state.set(AdvanceState::Loading);
        self.bump_load_generation();
        load_current(self.clone());
    }

    /// Preferred page-facing name: play a list context from an index.
    pub fn play_context(&self, tracks: Vec<Track>, start_idx: usize) {
        self.play_list(tracks, start_idx);
    }

    /// Play exactly one track; mostly for cards that have no surrounding
    /// context. Pages should prefer `play_context` when a list exists.
    pub fn play_track(&self, track: Track) {
        self.play_list(vec![track], 0);
    }

    pub fn next(&self) {
        let cur = *self.current_index.peek();
        let entries = self.entries.peek();
        let Some(i) = cur else { return };
        let len = entries.len();
        drop(entries);

        let next_idx = if i + 1 >= len {
            if *self.repeat_mode.peek() == RepeatMode::All {
                0
            } else {
                return;
            }
        } else {
            i + 1
        };

        self.jump_to(next_idx);
    }

    pub fn advance_after_end(&self) {
        let cur = *self.current_index.peek();
        let entries = self.entries.peek();
        let Some(i) = cur else { return };
        let len = entries.len();
        drop(entries);

        let next_idx = if *self.repeat_mode.peek() == RepeatMode::One {
            i
        } else if i + 1 >= len {
            match *self.repeat_mode.peek() {
                RepeatMode::All => 0,
                RepeatMode::Off | RepeatMode::One => {
                    self.stop();
                    return;
                }
            }
        } else {
            i + 1
        };

        self.jump_to(next_idx);
    }

    fn jump_to(&self, idx: usize) {
        let mut current = self.current_index;
        let mut state = self.advance_state;
        current.set(Some(idx));
        state.set(AdvanceState::Loading);
        self.bump_load_generation();
        load_current(self.clone());
    }

    pub fn previous(&self) {
        let cur = *self.current_index.peek();
        let entries = self.entries.peek();
        let Some(i) = cur else { return };
        let len = entries.len();
        drop(entries);

        let prev_idx = if i == 0 {
            if *self.repeat_mode.peek() == RepeatMode::All && len > 1 {
                len - 1
            } else {
                return;
            }
        } else {
            i - 1
        };

        self.jump_to(prev_idx);
    }

    pub fn stop(&self) {
        let mut state = self.advance_state;
        let mut current = self.current_index;
        let mut entries = self.entries;
        // Mark Idle *before* hitting the player so the watcher can't catch
        // the `has_source=false` transition and try to auto-advance.
        state.set(AdvanceState::Idle);
        current.set(None);
        entries.set(Vec::new());
        self.bump_load_generation();
        let mut is_loading = self.is_loading_track;
        is_loading.set(false);
        self.player.stop();
    }

    /// Append a track to the end of the queue. Doesn't touch playback. If
    /// the queue is empty, the appended track does *not* auto-play —
    /// "Add to queue" is the patient action; users press a separate Play to
    /// start it. (Compare `play_list` which forces playback.)
    pub fn add_to_queue(&self, track: Track) {
        let mut entries = self.entries;
        let mut updated = entries.peek().clone();
        updated.push(track);
        entries.set(updated);
    }

    /// Insert a track right after the currently-playing entry so it plays
    /// next when the current one ends (or when the user hits Next). If
    /// nothing is playing yet, behaves like `add_to_queue`.
    pub fn play_next(&self, track: Track) {
        let cur = *self.current_index.peek();
        let mut entries = self.entries;
        let mut updated = entries.peek().clone();
        match cur {
            Some(i) if i < updated.len() => updated.insert(i + 1, track),
            _ => updated.push(track),
        }
        entries.set(updated);
    }

    /// Kick off a "Song Radio": fetches ~40 tracks similar to `seed`
    /// from the discovery engine using the configured source mix, prepends
    /// the seed itself, and replaces the queue with the result.
    /// Runs async because the lookup takes 1–3 s; the caller closes the
    /// menu immediately. On lookup failure we still play the seed alone —
    /// at least the user-clicked track plays. A separate `radio_status`
    /// signal carries loading/error so a future toast UI can subscribe.
    pub fn start_song_radio(&self, seed: Track, engine: Arc<DiscoveryEngine>) {
        let queue = self.clone();
        let mut status = self.radio_status;
        status.set(RadioStatus::Loading);
        let seed_for_fallback = seed.clone();
        spawn(async move {
            let s = SimilarToSeed {
                artist: seed
                    .artists
                    .first()
                    .map(|a| a.name.clone())
                    .unwrap_or_default(),
                title: seed.title.clone(),
                mbid: None,
            };
            let results = match engine.similar_to(s).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "song radio lookup failed");
                    status.set(RadioStatus::Error(e.to_string()));
                    // Still play the seed so the user doesn't get nothing.
                    queue.play_list(vec![seed_for_fallback], 0);
                    return;
                }
            };
            let mut list = Vec::with_capacity(results.len() + 1);
            list.push(seed);
            for r in results.iter().take(40) {
                if let Some(t) = r.play_target() {
                    list.push(t);
                }
            }
            tracing::info!(count = list.len(), "song radio queued");
            status.set(RadioStatus::Idle);
            queue.play_list(list, 0);
        });
    }

    pub fn toggle_shuffle(&self) {
        let next = !*self.shuffle_enabled.peek();
        let mut shuffle = self.shuffle_enabled;
        shuffle.set(next);
        if !next {
            return;
        }

        let cur = *self.current_index.peek();
        let Some(i) = cur else { return };
        let entries_now = self.entries.peek().clone();
        if entries_now.len() <= 2 || i >= entries_now.len() {
            return;
        }
        let (shuffled, idx) = shuffled_context(entries_now, i);
        let mut entries = self.entries;
        let mut current = self.current_index;
        entries.set(shuffled);
        current.set(Some(idx));
        if *self.advance_state.peek() == AdvanceState::Loading {
            self.bump_load_generation();
            load_current(self.clone());
        }
    }

    pub fn cycle_repeat(&self) {
        let next = match *self.repeat_mode.peek() {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        let mut repeat = self.repeat_mode;
        repeat.set(next);
    }

    pub fn has_next(&self) -> bool {
        if *self.repeat_mode.peek() == RepeatMode::All {
            return self.current_index.peek().is_some();
        }
        let entries = self.entries.peek();
        let cur = *self.current_index.peek();
        match cur {
            Some(i) => i + 1 < entries.len(),
            None => false,
        }
    }

    pub fn has_previous(&self) -> bool {
        if *self.repeat_mode.peek() == RepeatMode::All {
            return self.entries.peek().len() > 1 && self.current_index.peek().is_some();
        }
        matches!(*self.current_index.peek(), Some(i) if i > 0)
    }

    fn bump_load_generation(&self) -> u64 {
        let mut generation = self.load_generation;
        let next = generation.peek().wrapping_add(1);
        generation.set(next);
        next
    }
}

fn shuffled_context(mut tracks: Vec<Track>, start_idx: usize) -> (Vec<Track>, usize) {
    if tracks.len() <= 2 || start_idx >= tracks.len() {
        return (tracks, start_idx);
    }
    let current = tracks.remove(start_idx);
    tracks.shuffle(&mut rand::rng());
    tracks.insert(0, current);
    (tracks, 0)
}

/// Spawn the load task for the queue's current_index entry. Used by
/// play_list / next / previous and by the auto-advance watcher.
fn load_current(queue: UseQueue) {
    let generation = *queue.load_generation.peek();
    let Some(idx) = *queue.current_index.peek() else {
        return;
    };
    let entries = queue.entries.peek().clone();
    let Some(track) = entries.get(idx).cloned() else {
        return;
    };
    let expected_uri = track.uri.0.clone();

    let sc = queue.sc.clone();
    let sp = queue.sp.clone();
    let player = queue.player.clone();
    let mut error = queue.error;
    let mut is_loading = queue.is_loading_track;

    spawn(async move {
        is_loading.set(true);
        error.set(None);
        player.set_now_playing(Some(now_playing_from(&track)));

        let outcome: Result<(), String> = match track.provider {
            ProviderId::SoundCloud => match load_sc(sc.as_ref(), &track).await {
                Ok(bytes) => {
                    if !is_current_load(&queue, generation, idx, &expected_uri) {
                        return;
                    }
                    player.play_bytes(bytes).map_err(|e| format!("decode: {e}"))
                }
                Err(msg) => Err(msg),
            },
            ProviderId::Spotify => match sp.access_token_for_playback().await {
                Err(e) => Err(format!("spotify auth: {e}")),
                Ok(token) => {
                    if let Err(e) = player.ensure_spotify(&token).await {
                        Err(format!("librespot connect: {e}"))
                    } else {
                        if !is_current_load(&queue, generation, idx, &expected_uri) {
                            return;
                        }
                        player
                            .play_spotify(&track.uri.0, Some(track.duration))
                            .map_err(|e| format!("librespot play: {e}"))
                    }
                }
            },
            ProviderId::Local => Err("Local files come back later.".into()),
        };

        if !is_current_load(&queue, generation, idx, &expected_uri) {
            return;
        }

        if let Err(msg) = outcome {
            let has_followup = idx + 1 < queue.entries.peek().len();
            if matches!(track.provider, ProviderId::SoundCloud)
                && is_sc_unavailable_message(&msg)
                && has_followup
            {
                tracing::warn!(%msg, "skipping unavailable SoundCloud queue entry");
                error.set(Some("Skipped unavailable SoundCloud track.".into()));
                queue.next();
                return;
            }
            error.set(Some(msg));
            player.set_now_playing(None);
            // A failed load shouldn't strand the watcher in Loading — drop
            // to Idle so a manual `next()` from the bottombar still works.
            let mut state = queue.advance_state;
            state.set(AdvanceState::Idle);
        }
        is_loading.set(false);
    });
}

fn is_current_load(queue: &UseQueue, generation: u64, idx: usize, uri: &str) -> bool {
    if *queue.load_generation.peek() != generation {
        return false;
    }
    if *queue.current_index.peek() != Some(idx) {
        return false;
    }
    queue
        .entries
        .peek()
        .get(idx)
        .is_some_and(|track| track.uri.0 == uri)
}

fn now_playing_from(track: &Track) -> NowPlaying {
    let artist = track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    NowPlaying {
        title: track.title.clone(),
        artist,
        cover_url: track.cover_url.clone(),
        source_label: track.provider.label().to_lowercase(),
        provider: track.provider.label().to_string(),
        track_uri: Some(track.uri.0.clone()),
    }
}

async fn load_sc(sc: &SoundCloudProvider, track: &Track) -> Result<Vec<u8>, String> {
    let stream = match sc.resolve_stream(&track.uri).await {
        Ok(stream) => stream,
        Err(e) if is_sc_not_found(&e.to_string()) => {
            tracing::warn!(error = %e, "SoundCloud stream resolve failed; refreshing client_id");
            let _ = sc.refresh_client_id().await;
            sc.resolve_stream(&track.uri)
                .await
                .map_err(|e| friendly_sc_error(&e.to_string()))?
        }
        Err(e) => return Err(friendly_sc_error(&e.to_string())),
    };
    match stream {
        // Progressive: a single CDN URL — fetch and hand back.
        StreamHandle::HttpStream { url, .. } => {
            let resp = reqwest::get(&url)
                .await
                .map_err(|_| "SoundCloud download failed. Try again later.".to_string())?;
            if !resp.status().is_success() {
                return Err("SoundCloud stream is unavailable. Try the track again or refresh SoundCloud in Settings.".into());
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|_| "SoundCloud download failed. Try again later.".to_string())?;
            Ok(bytes.to_vec())
        }
        // HLS: the provider already resolved + concatenated all segments
        // because no single URL covers the audio. Skip the second fetch.
        StreamHandle::Bytes { data, .. } => Ok(data),
        StreamHandle::InProcess { .. } => Err("unexpected stream variant".into()),
    }
}

fn is_sc_not_found(raw: &str) -> bool {
    raw.contains("404") || raw.contains("Not Found")
}

fn is_sc_unavailable_message(msg: &str) -> bool {
    msg.contains("SoundCloud track is unavailable")
        || msg.contains("SoundCloud stream is unavailable")
}

fn friendly_sc_error(raw: &str) -> String {
    if is_sc_not_found(raw) {
        "SoundCloud track is unavailable. Try again or refresh SoundCloud in Settings.".into()
    } else if raw.contains("401") || raw.contains("Unauthorized") || raw.contains("auth") {
        "SoundCloud session expired. Refresh SoundCloud in Settings.".into()
    } else {
        "SoundCloud playback failed. Try again later.".into()
    }
}

/// Install queue signals into Dioxus context and spawn the auto-advance
/// watcher. Called once from `AppContext::install`.
pub fn install(player: Player, sc: Arc<SoundCloudProvider>, sp: Arc<SpotifyProvider>) {
    let entries = use_signal(Vec::<Track>::new);
    let current_index = use_signal(|| None::<usize>);
    let shuffle_enabled = use_signal(|| false);
    let repeat_mode = use_signal(|| RepeatMode::Off);
    let advance_state = use_signal(|| AdvanceState::Idle);
    let load_generation = use_signal(|| 0u64);
    let error = use_signal(|| None::<String>);
    let is_loading_track = use_signal(|| false);
    let radio_status = use_signal(|| RadioStatus::Idle);

    let queue = UseQueue {
        entries,
        current_index,
        shuffle_enabled,
        repeat_mode,
        error,
        is_loading_track,
        advance_state,
        load_generation,
        radio_status,
        sc,
        sp,
        player: player.clone(),
    };
    use_context_provider({
        let queue = queue.clone();
        move || queue
    });

    // Watcher — polls player.snapshot, drives the small state machine,
    // triggers auto-advance on natural track-end.
    use_hook(move || {
        let queue_for_watcher = queue.clone();
        let player_for_watcher = player.clone();
        spawn(async move {
            let queue = queue_for_watcher;
            let player = player_for_watcher;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let snap = player.snapshot();
                let mut state = queue.advance_state;
                let current = *state.peek();

                match current {
                    AdvanceState::Loading if snap.has_source => {
                        tracing::info!("queue: load succeeded, now Playing");
                        state.set(AdvanceState::Playing);
                    }
                    AdvanceState::Playing if !snap.has_source && !snap.is_paused => {
                        // Backend dropped its source while we expected it to
                        // be playing. Treat as a natural end and advance.
                        let will_advance =
                            queue.has_next() || *queue.repeat_mode.peek() != RepeatMode::Off;
                        tracing::info!(
                            will_advance,
                            repeat_mode = ?queue.repeat_mode.peek(),
                            current_index = ?queue.current_index.peek(),
                            entries = queue.entries.peek().len(),
                            "queue: track ended"
                        );
                        queue.advance_after_end();
                    }
                    _ => {
                        // Periodic trace at debug level so users can verify
                        // the watcher is alive without log-spam by default.
                        tracing::debug!(
                            ?current,
                            has_source = snap.has_source,
                            is_paused = snap.is_paused,
                            "queue tick"
                        );
                    }
                }
            }
        });

        // Transport-bus consumer — drains TransportCmd::Next/Previous emitted
        // by MPRIS (Linux media keys, playerctl, KDE/GNOME widgets) and walks
        // the queue accordingly. The Mutex-Option dance in `take_transport_rx`
        // means this only attaches once even if `install` were ever called
        // twice; subsequent calls would yield None and silently no-op.
        if let Some(mut rx) = player.take_transport_rx() {
            let queue = queue.clone();
            spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        TransportCmd::Next => queue.next(),
                        TransportCmd::Previous => queue.previous(),
                        TransportCmd::Stop => queue.stop(),
                    }
                }
            });
        }
    });
}

pub fn use_queue() -> UseQueue {
    use_context::<UseQueue>()
}
