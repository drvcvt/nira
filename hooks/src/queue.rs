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

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use discovery::{DiscoveryEngine, DiscoveryResult, SimilarToSeed, canonical_title};
use player::{Active, NowPlaying, Player, PlayerSnapshot, TransportCmd};
use provider_api::{Provider, ProviderId, StreamHandle, Track};
use provider_hires-provider::the hi-res providerProvider;
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;
use rand::Rng;
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
    /// The queue order from before shuffle was switched on, so switching it
    /// off restores the original order instead of leaving the scramble.
    /// Kept in sync by add_to_queue/play_next while shuffled.
    pre_shuffle: Signal<Option<Vec<Track>>>,
    pub radio_status: Signal<RadioStatus>,
    /// Gapless bookkeeping: (load_generation, target index) of the entry
    /// whose audio has been (or is being) appended to the sink for a
    /// gapless hand-off. Cleared when the hand-off commits or fails.
    gapless_prefetched: Signal<Option<(u64, usize)>>,
    /// One-shot mid-track resume: (entry uri, position) restored from disk.
    /// Consumed on the first successful load; only seeks when that load is
    /// the entry the position belongs to.
    resume_hint: Signal<Option<(String, Duration)>>,
    sc: Arc<SoundCloudProvider>,
    sp: Arc<SpotifyProvider>,
    qz: Arc<the hi-res providerProvider>,
    player: Player,
    /// FLAC-first swap cache: original track URI → the matched the hi-res provider track
    /// (None = searched, no strict match). Session-scoped so replays and
    /// queue walks don't re-hit the the hi-res provider search API per play. Errors are
    /// NOT cached — a network blip shouldn't pin a track to lossy forever.
    qz_swaps: Arc<std::sync::Mutex<std::collections::HashMap<String, Option<Track>>>>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

/// On-disk shape of the queue (cache tier). Written on every queue-shape
/// change, read once on boot so a restart resumes where the session left
/// off — paused, at the same entry; pressing play starts it.
#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedQueue {
    entries: Vec<Track>,
    current_index: Option<usize>,
    shuffle: bool,
    repeat: RepeatMode,
}

/// Companion file to [`PersistedQueue`]: where inside the current entry
/// playback was, written every ~5 s while playing. Restoring arms a
/// one-shot seek so pressing play continues mid-track.
#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedPosition {
    uri: String,
    secs: u64,
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
        let mut pre_shuffle = self.pre_shuffle;
        let (tracks, idx) = if *self.shuffle_enabled.peek() {
            pre_shuffle.set(Some(tracks.clone()));
            shuffled_context(tracks, idx)
        } else {
            pre_shuffle.set(None);
            (tracks, idx)
        };
        let mut entries = self.entries;
        let mut current = self.current_index;
        let mut state = self.advance_state;
        entries.set(tracks);
        current.set(Some(idx));
        state.set(AdvanceState::Loading);
        // Silence the outgoing track before the (multi-second) load — the
        // watcher must never see the old source's `has_source=true` while
        // we're in Loading, or it mistakes it for the new track committing.
        self.player.stop_for_load();
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

    /// Play `tracks` shuffled from a random starting point, switching the
    /// queue's shuffle mode on — the Library "Shuffle all" action.
    pub fn shuffle_all(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let mut shuffle = self.shuffle_enabled;
        shuffle.set(true);
        let idx = rand::rng().random_range(0..tracks.len());
        self.play_list(tracks, idx);
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
                    self.finish_playback();
                    return;
                }
            }
        } else {
            i + 1
        };

        self.jump_to(next_idx);
    }

    /// Natural end of the queue: stop the audio but KEEP the queue —
    /// wiping it on completion made "play it again" impossible. The index
    /// winds back to the start so the play button restarts from the top.
    fn finish_playback(&self) {
        let mut state = self.advance_state;
        state.set(AdvanceState::Idle);
        self.bump_load_generation();
        let mut is_loading = self.is_loading_track;
        is_loading.set(false);
        let mut current = self.current_index;
        current.set(Some(0));
        self.player.stop();
        // The queue ran out naturally — a stale mid-track position would
        // otherwise resurface on the next boot's "play it again". Routed
        // through the writer queue so a position write enqueued seconds ago
        // can't land after this delete and resurrect the file.
        if let Some(p) = config::AppConfig::playback_position_path() {
            let _ = config::AppConfig::remove_bg(p);
        }
    }

    fn jump_to(&self, idx: usize) {
        let mut current = self.current_index;
        let mut state = self.advance_state;
        current.set(Some(idx));
        state.set(AdvanceState::Loading);
        // Same old-source silencing as play_list — see comment there.
        self.player.stop_for_load();
        self.bump_load_generation();
        load_current(self.clone());
    }

    /// Jump to an existing queue entry. Unlike `play_list`, this neither
    /// re-seeds nor re-shuffles the queue — it's what the queue popover and
    /// the idle-play button use to start playback *within* the current queue.
    pub fn play_index(&self, idx: usize) {
        let len = self.entries.peek().len();
        if len == 0 {
            return;
        }
        self.jump_to(idx.min(len - 1));
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

    /// Stop playback. The queue itself survives — a stop (MPRIS, future UI)
    /// shouldn't destroy what the user lined up; `clear_upcoming` is the
    /// destructive action.
    pub fn stop(&self) {
        let mut state = self.advance_state;
        // Mark Idle *before* hitting the player so the watcher can't catch
        // the `has_source=false` transition and try to auto-advance.
        state.set(AdvanceState::Idle);
        self.bump_load_generation();
        let mut is_loading = self.is_loading_track;
        is_loading.set(false);
        self.player.stop();
    }

    /// Drop everything except the currently-playing entry; playback keeps
    /// running. This is the queue popover's "clear" action.
    pub fn clear_upcoming(&self) {
        self.invalidate_gapless();
        let entries_now = self.entries.peek().clone();
        let cur = *self.current_index.peek();
        let was_loading = *self.advance_state.peek() == AdvanceState::Loading;
        let mut entries = self.entries;
        let mut current = self.current_index;
        let mut pre_shuffle = self.pre_shuffle;
        pre_shuffle.set(None);
        match cur.and_then(|i| entries_now.get(i).cloned()) {
            Some(track) => {
                entries.set(vec![track]);
                current.set(Some(0));
                // An in-flight load was keyed to the old index — re-issue it
                // against the new single-entry queue or it bails on commit.
                if was_loading {
                    self.bump_load_generation();
                    load_current(self.clone());
                }
            }
            None => {
                entries.set(Vec::new());
                current.set(None);
            }
        }
    }

    /// Append a track to the end of the queue. Doesn't touch playback. If
    /// the queue is empty, the appended track does *not* auto-play —
    /// "Add to queue" is the patient action; users press a separate Play to
    /// start it. (Compare `play_list` which forces playback.)
    pub fn add_to_queue(&self, track: Track) {
        self.invalidate_gapless();
        let mut entries = self.entries;
        let mut updated = entries.peek().clone();
        updated.push(track.clone());
        entries.set(updated);
        self.append_to_pre_shuffle(track);
    }

    /// Append several tracks (an album drop) in one signal write. Same
    /// patient semantics as `add_to_queue` — never starts playback.
    pub fn add_all(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        self.invalidate_gapless();
        let mut entries = self.entries;
        let mut updated = entries.peek().clone();
        updated.extend(tracks.iter().cloned());
        entries.set(updated);
        for t in tracks {
            self.append_to_pre_shuffle(t);
        }
    }

    /// A queue edit invalidates any queued gapless hand-off — its audio may
    /// belong to an entry that no longer follows the current one. The player
    /// skips the stale audio at the boundary; the prefetch window re-arms
    /// against the edited queue on the next watcher tick.
    fn invalidate_gapless(&self) {
        if self.gapless_prefetched.peek().is_some() {
            self.player.cancel_next();
            let mut prefetched = self.gapless_prefetched;
            prefetched.set(None);
        }
    }

    /// Remove one entry from the queue. Removing the playing entry loads
    /// whatever slides into its slot (or stops on an emptied queue); rows
    /// before the playing one shift the index down by one.
    pub fn remove_at(&self, idx: usize) {
        let mut updated = self.entries.peek().clone();
        if idx >= updated.len() {
            return;
        }
        self.invalidate_gapless();
        let removed = updated.remove(idx);
        let cur = *self.current_index.peek();
        let mut entries = self.entries;
        let mut current = self.current_index;

        // Keep the pre-shuffle order in sync so switching shuffle off
        // doesn't resurrect the removed row.
        let mut pre_shuffle = self.pre_shuffle;
        let saved_order = pre_shuffle.peek().clone();
        if let Some(mut original) = saved_order
            && let Some(p) = original.iter().position(|t| t.uri == removed.uri)
        {
            original.remove(p);
            pre_shuffle.set(Some(original));
        }

        match cur {
            Some(c) if idx == c => {
                if updated.is_empty() {
                    entries.set(updated);
                    current.set(None);
                    self.stop();
                    return;
                }
                let new_idx = c.min(updated.len() - 1);
                entries.set(updated);
                current.set(Some(new_idx));
                // Only reload when audio was actually going — removing the
                // pointed-at row of an idle (restored) queue just re-points.
                if *self.advance_state.peek() != AdvanceState::Idle {
                    let mut state = self.advance_state;
                    state.set(AdvanceState::Loading);
                    self.player.stop_for_load();
                    self.bump_load_generation();
                    load_current(self.clone());
                }
            }
            Some(c) if idx < c => {
                entries.set(updated);
                current.set(Some(c - 1));
            }
            _ => entries.set(updated),
        }
    }

    /// Insert a track right after the currently-playing entry so it plays
    /// next when the current one ends (or when the user hits Next). If
    /// nothing is playing yet, behaves like `add_to_queue`.
    pub fn play_next(&self, track: Track) {
        self.invalidate_gapless();
        let cur = *self.current_index.peek();
        let mut entries = self.entries;
        let mut updated = entries.peek().clone();
        match cur {
            Some(i) if i < updated.len() => updated.insert(i + 1, track.clone()),
            _ => updated.push(track.clone()),
        }
        entries.set(updated);
        self.append_to_pre_shuffle(track);
    }

    /// Keep the saved pre-shuffle order in sync with tracks queued while
    /// shuffle is on, so switching shuffle off doesn't drop them.
    fn append_to_pre_shuffle(&self, track: Track) {
        let mut pre_shuffle = self.pre_shuffle;
        let updated = pre_shuffle.peek().clone().map(|mut v| {
            v.push(track);
            v
        });
        if let Some(v) = updated {
            pre_shuffle.set(Some(v));
        }
    }

    /// Kick off a "Song Radio": fetches ~40 tracks similar to `seed`
    /// from the discovery engine using the configured source mix, prepends
    /// the seed itself, and replaces the queue with the result.
    /// Runs async because the lookup takes 1–3 s; the caller closes the
    /// menu immediately. On lookup failure we still play the seed alone —
    /// at least the user-clicked track plays. `radio_status` carries
    /// loading/error; the bottombar renders it as a toast.
    pub fn start_song_radio(&self, seed: Track, engine: Arc<DiscoveryEngine>) {
        let queue = self.clone();
        let mut status = self.radio_status;
        status.set(RadioStatus::Loading);
        let seed_for_fallback = seed.clone();
        // Spotify-style blend: a second seed drawn from recent listening
        // (decay-weighted, different artist) widens the radio beyond the
        // clicked track's immediate neighbourhood, 2:1 in the seed's favour.
        // ponytail: one profile seed; bump to 2-3 if radios still feel narrow.
        let profile_seed = self.radio_profile_seed(&seed);
        // The lookup takes seconds; if the user starts anything else in the
        // meantime (click a track, next, stop), every one of those bumps the
        // load generation — a changed generation means the radio result must
        // be dropped instead of wiping whatever the user chose to play.
        let generation_at_start = *self.load_generation.peek();
        // Root-scoped like load_current: radio starts from page/ctx-menu
        // scopes, and an unmount mid-lookup would strand RadioStatus::Loading.
        spawn_forever(async move {
            let s = SimilarToSeed {
                artist: seed
                    .artists
                    .first()
                    .map(|a| a.name.clone())
                    .unwrap_or_default(),
                title: seed.title.clone(),
                mbid: None,
            };
            let superseded = |q: &UseQueue| *q.load_generation.peek() != generation_at_start;
            // The profile path is best-effort: its failure never fails the
            // radio, the seed's own neighbourhood just plays undiluted.
            let looked_up = match profile_seed {
                Some(p) => {
                    let (main, extra) = tokio::join!(engine.similar_to(s), engine.similar_to(p));
                    main.map(|m| match extra {
                        Ok(e) => interleave_radio(m, e),
                        Err(_) => m,
                    })
                }
                None => engine.similar_to(s).await,
            };
            let results = match looked_up {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "song radio lookup failed");
                    if superseded(&queue) {
                        status.set(RadioStatus::Idle);
                        return;
                    }
                    status.set(RadioStatus::Error(e.to_string()));
                    // Seed-alone fallback ONLY into an empty queue — a
                    // failed lookup must never eat an existing queue (that
                    // read as "my radio songs just vanished").
                    if queue.entries.peek().is_empty() {
                        queue.play_list(vec![seed_for_fallback], 0);
                    }
                    return;
                }
            };
            if superseded(&queue) {
                status.set(RadioStatus::Idle);
                return;
            }
            let mut list = Vec::with_capacity(results.len() + 1);
            list.push(seed);
            for r in results.iter().take(40) {
                if let Some(t) = r.play_target() {
                    list.push(t);
                }
            }
            // Keep the clicked seed at 0 but pull same-artist runs apart.
            spread_artists(&mut list);
            tracing::info!(count = list.len(), "song radio queued");
            status.set(RadioStatus::Idle);
            queue.play_list(list, 0);
        });
    }

    /// A second radio seed sampled from recent listening: decay-weighted so
    /// current taste dominates, restricted to artists other than the radio
    /// seed itself. Repeat plays of a track naturally stack its weight.
    fn radio_profile_seed(&self, seed: &Track) -> Option<SimilarToSeed> {
        let seed_artist = seed
            .artists
            .first()
            .map(|a| a.name.to_lowercase())
            .unwrap_or_default();
        let now = chrono::Utc::now();
        let pool: Vec<(f64, SimilarToSeed)> = self
            .player
            .history()
            .recent(50)
            .into_iter()
            .filter(|e| {
                !e.artist.is_empty()
                    && !e.title.is_empty()
                    && e.artist.to_lowercase() != seed_artist
            })
            .map(|e| {
                (
                    crate::taste::play_weight(now, e.played_at),
                    SimilarToSeed {
                        artist: e.artist,
                        title: e.title,
                        mbid: None,
                    },
                )
            })
            .collect();
        crate::taste::weighted_sample(pool, 1, &mut rand::rng())
            .into_iter()
            .next()
    }

    pub fn toggle_shuffle(&self) {
        // Re-ordering moves the "next" slot — any queued hand-off is stale.
        self.invalidate_gapless();
        let next = !*self.shuffle_enabled.peek();
        let mut shuffle = self.shuffle_enabled;
        shuffle.set(next);
        if !next {
            // Off: bring back the order from before shuffling (plus anything
            // queued meanwhile), keeping the playing track selected.
            self.restore_pre_shuffle();
            return;
        }

        let cur = *self.current_index.peek();
        let Some(i) = cur else { return };
        let entries_now = self.entries.peek().clone();
        if entries_now.len() <= 2 || i >= entries_now.len() {
            return;
        }
        let mut pre_shuffle = self.pre_shuffle;
        pre_shuffle.set(Some(entries_now.clone()));
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

    /// Undo the shuffle scramble: restore the saved order, keep tracks that
    /// were queued while shuffled (appended at the end), and re-point
    /// `current_index` at the entry that is actually playing.
    fn restore_pre_shuffle(&self) {
        let mut pre_shuffle = self.pre_shuffle;
        let Some(original) = pre_shuffle.peek().clone() else {
            return;
        };
        pre_shuffle.set(None);
        let entries_now = self.entries.peek().clone();
        if entries_now.is_empty() {
            return;
        }
        // Multiset diff by URI: whatever the current queue holds beyond the
        // saved order was added while shuffled — keep it.
        let mut budget = std::collections::HashMap::<&str, usize>::new();
        for t in &original {
            *budget.entry(t.uri.0.as_str()).or_default() += 1;
        }
        let mut extra = Vec::new();
        for t in &entries_now {
            match budget.get_mut(t.uri.0.as_str()) {
                Some(n) if *n > 0 => *n -= 1,
                _ => extra.push(t.clone()),
            }
        }
        let current_uri = self
            .current_index
            .peek()
            .and_then(|i| entries_now.get(i).map(|t| t.uri.0.clone()));
        let mut merged = original;
        merged.extend(extra);
        let idx = current_uri.and_then(|uri| merged.iter().position(|t| t.uri.0 == uri));
        let mut entries = self.entries;
        let mut current = self.current_index;
        entries.set(merged);
        if let Some(i) = idx {
            current.set(Some(i));
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
    spread_artists(&mut tracks);
    (tracks, 0)
}

fn spread_key(track: &Track) -> String {
    track
        .artists
        .first()
        .map(|a| a.name.to_lowercase())
        .unwrap_or_default()
}

/// Post-shuffle pass, Spotify-style: pull back-to-back same-artist pairs
/// apart by swapping the second one with the next differing track. Index 0
/// (the playing track) is never moved.
/// ponytail: greedy single pass, O(n²) worst case — fine at queue sizes;
/// a proper balanced-shuffle only if runs still bother anyone.
fn spread_artists(tracks: &mut [Track]) {
    for i in 1..tracks.len() {
        if spread_key(&tracks[i]) != spread_key(&tracks[i - 1]) {
            continue;
        }
        if let Some(j) =
            (i + 1..tracks.len()).find(|&j| spread_key(&tracks[j]) != spread_key(&tracks[i - 1]))
        {
            tracks.swap(i, j);
        }
    }
}

/// Blend two radio result sets 2:1 — two picks from the clicked seed's
/// neighbourhood, then one from the taste-profile seed — deduped on the
/// noise-stripped title so the same song can't enter once per source.
fn interleave_radio(
    main: Vec<DiscoveryResult>,
    profile: Vec<DiscoveryResult>,
) -> Vec<DiscoveryResult> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(main.len() + profile.len());
    let mut push = |r: DiscoveryResult, out: &mut Vec<DiscoveryResult>| {
        let canon = canonical_title(&r.title);
        let key = if canon.is_empty() {
            format!("{}|{}", r.artist.to_lowercase(), r.title.to_lowercase())
        } else {
            canon
        };
        if seen.insert(key) {
            out.push(r);
        }
    };
    let mut main = main.into_iter();
    let mut profile = profile.into_iter();
    loop {
        let mut any = false;
        for _ in 0..2 {
            if let Some(r) = main.next() {
                any = true;
                push(r, &mut out);
            }
        }
        if let Some(r) = profile.next() {
            any = true;
            push(r, &mut out);
        }
        if !any {
            break;
        }
    }
    out
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

    let player = queue.player.clone();
    let mut error = queue.error;
    let mut is_loading = queue.is_loading_track;

    // Root-scoped on purpose: track clicks spawn this from detail-page
    // scopes, and a scope-tied `spawn` dies with the page — Back during the
    // load left the queue stranded in Loading with the audio already
    // silenced. Staleness is handled by the generation checks, not by
    // cancellation.
    spawn_forever(async move {
        is_loading.set(true);
        error.set(None);
        player.set_now_playing(Some(now_playing_from(&track)));

        // FLAC first: streaming-provider tracks get one shot at resolving
        // the same recording on the hi-res provider (strict artist+title+duration match).
        // Hit → we stream the FLAC instead and the player bar shows the hi-res provider
        // as the source; miss or any error → the original provider plays.
        // The queue entry keeps its original identity (likes, re-clicks).
        let swap = resolve_flac_first(&queue, &track).await;
        let playing = match &swap {
            Some(qz_track) => {
                if !is_current_load(&queue, generation, idx, &expected_uri) {
                    return;
                }
                tracing::info!(from = %track.uri.0, to = %qz_track.uri.0, "flac-first: playing the hi-res provider variant");
                player.set_now_playing(Some(now_playing_from(qz_track)));
                qz_track.clone()
            }
            None => track.clone(),
        };

        let Some(mut outcome) =
            play_one(&queue, &player, &playing, generation, idx, &expected_uri).await
        else {
            return; // superseded by a newer load
        };

        // The matched FLAC resolved but wouldn't stream (region lock, sub
        // limits, CDN hiccup) — retry once via the original provider so the
        // user still hears the track instead of an error.
        if outcome.is_err() && swap.is_some() {
            tracing::warn!(
                error = ?outcome.as_ref().err(),
                "flac-first: hires-provider stream failed, falling back to original provider"
            );
            player.set_now_playing(Some(now_playing_from(&track)));
            let Some(retry) =
                play_one(&queue, &player, &track, generation, idx, &expected_uri).await
            else {
                return;
            };
            outcome = retry;
        }

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

/// How close to the end of the current track the gapless prefetch kicks in.
/// Enough headroom for a stream URL resolve + HTTP prefetch on a normal
/// connection; short enough that skips rarely waste a fetch.
const GAPLESS_PREFETCH_WINDOW: Duration = Duration::from_secs(12);

/// Near the end of a rodio-backed track, resolve the next queue entry and
/// append its audio to the sink so the hand-off is gapless. Streaming
/// entries get the same FLAC-first swap as a normal load. Every failure is
/// silent — the falling-edge auto-advance handles the transition exactly as
/// before, just with the old audible gap. Queue edits cancel a queued
/// hand-off via `invalidate_gapless` (the player skips the stale audio).
fn maybe_prefetch_gapless(queue: &UseQueue, snap: &PlayerSnapshot) {
    if snap.active != Active::Rodio || snap.is_paused {
        return;
    }
    let Some(dur) = snap.duration else { return };
    if dur.is_zero() || snap.position >= dur || dur - snap.position > GAPLESS_PREFETCH_WINDOW {
        return;
    }
    let Some(i) = *queue.current_index.peek() else {
        return;
    };
    let len = queue.entries.peek().len();
    // Mirror advance_after_end's target choice so repeat modes loop
    // gaplessly too.
    let target = match *queue.repeat_mode.peek() {
        RepeatMode::One => i,
        RepeatMode::All if i + 1 >= len => 0,
        _ if i + 1 < len => i + 1,
        _ => return, // queue ends here — natural stop
    };
    let generation = *queue.load_generation.peek();
    if *queue.gapless_prefetched.peek() == Some((generation, target)) {
        return; // already in flight / appended for this transition
    }
    let mut prefetched = queue.gapless_prefetched;
    prefetched.set(Some((generation, target)));
    spawn(prefetch_gapless(queue.clone(), generation, target));
}

async fn prefetch_gapless(queue: UseQueue, generation: u64, target: usize) {
    let Some(track) = queue.entries.peek().get(target).cloned() else {
        return;
    };
    // librespot is its own engine — a Spotify entry can't be appended to
    // the rodio sink; that transition keeps the normal load path.
    if track.provider == ProviderId::Spotify {
        return;
    }
    let playing = resolve_flac_first(&queue, &track)
        .await
        .unwrap_or_else(|| track.clone());
    // Superseded by a newer load OR cancelled by a queue edit — bail before
    // touching the sink.
    let stale = || {
        *queue.load_generation.peek() != generation
            || *queue.gapless_prefetched.peek() != Some((generation, target))
    };
    if stale() {
        return;
    }
    let np = now_playing_from(&playing);
    let player = queue.player.clone();
    let appended = match playing.provider {
        ProviderId::Local => match provider_local::path_from_uri(&playing.uri.0) {
            Some(path) => player
                .append_next_file(path, Some(playing.duration), np)
                .unwrap_or(false),
            None => false,
        },
        ProviderId::SoundCloud | ProviderId::the hi-res provider => {
            let loaded = if playing.provider == ProviderId::SoundCloud {
                load_sc(queue.sc.as_ref(), &playing).await
            } else {
                load_qz(queue.qz.as_ref(), &playing).await
            };
            match loaded {
                Ok(LoadedStream::Url(url)) => {
                    match Player::prepare_http(&url, Some(playing.duration)).await {
                        Ok(prepared) if !stale() => player.append_next_http(prepared, np),
                        _ => false,
                    }
                }
                Ok(LoadedStream::Bytes(bytes)) if !stale() => player
                    .append_next_bytes(bytes, Some(playing.duration), np)
                    .unwrap_or(false),
                _ => false,
            }
        }
        ProviderId::Spotify => false,
    };
    if appended {
        tracing::info!(target_idx = target, "gapless: next track appended to sink");
    } else {
        // Leave the marker set: one attempt per transition. Retrying every
        // watcher tick would re-resolve streams in a loop for the rest of
        // the window; the falling-edge advance covers the hand-off instead.
        tracing::debug!(target_idx = target, "gapless: prefetch not appended");
    }
}

/// Fetch + hand one track to the audio engine via its provider. Returns
/// `None` when the load was superseded mid-flight (newer generation, index
/// moved, entry replaced) — the caller must bail without touching state.
async fn play_one(
    queue: &UseQueue,
    player: &Player,
    track: &Track,
    generation: u64,
    idx: usize,
    expected_uri: &str,
) -> Option<Result<(), String>> {
    Some(match track.provider {
        ProviderId::SoundCloud => match load_sc(queue.sc.as_ref(), track).await {
            Ok(stream) => {
                match start_stream(queue, player, track, stream, generation, idx, expected_uri)
                    .await?
                {
                    // Keep SC errors in the wording the auto-skip logic knows.
                    Err(e) => Err(friendly_sc_error(&e)),
                    ok => ok,
                }
            }
            Err(msg) => Err(msg),
        },
        ProviderId::Spotify => match queue.sp.access_token_for_playback().await {
            Err(e) => Err(format!("spotify auth: {e}")),
            Ok(token) => {
                if let Err(e) = player.ensure_spotify(&token).await {
                    Err(format!("librespot connect: {e}"))
                } else {
                    if !is_current_load(queue, generation, idx, expected_uri) {
                        return None;
                    }
                    player
                        .play_spotify(&track.uri.0, Some(track.duration))
                        .map_err(|e| format!("librespot play: {e}"))
                }
            }
        },
        ProviderId::the hi-res provider => match load_qz(queue.qz.as_ref(), track).await {
            Ok(stream) => {
                start_stream(queue, player, track, stream, generation, idx, expected_uri).await?
            }
            Err(msg) => Err(msg),
        },
        ProviderId::Local => match provider_local::path_from_uri(&track.uri.0) {
            Some(path) => player
                .play_file(path, Some(track.duration))
                .map_err(|e| format!("Could not play local file: {e}")),
            None => Err("Malformed local track reference.".into()),
        },
    })
}

/// Resolve a Spotify/SoundCloud track to its the hi-res provider FLAC counterpart, if the
/// user is logged in to the hi-res provider and a strict match exists. Cached per original
/// URI for the session; search errors return None without caching so the
/// next play retries.
async fn resolve_flac_first(queue: &UseQueue, track: &Track) -> Option<Track> {
    if !matches!(
        track.provider,
        ProviderId::Spotify | ProviderId::SoundCloud
    ) {
        return None;
    }
    if !queue.qz.is_connected() {
        return None;
    }
    if let Ok(cache) = queue.qz_swaps.lock()
        && let Some(cached) = cache.get(&track.uri.0)
    {
        return cached.clone();
    }

    let artist = track
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let q = provider_api::Query {
        text: format!("{artist} {}", track.title).trim().to_string(),
        limit: Some(10),
    };
    let found = match queue.qz.search(&q).await {
        Ok(res) => crate::matching::find_strict_match(track, &res.tracks).cloned(),
        Err(e) => {
            tracing::debug!(error = %e, uri = %track.uri.0, "flac-first: hires-provider search failed");
            return None; // not cached — retry on next play
        }
    };
    if let Ok(mut cache) = queue.qz_swaps.lock() {
        cache.insert(track.uri.0.clone(), found.clone());
    }
    found
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

/// the hi-res provider streaming: getFileUrl → single FLAC CDN URL → bytes. The account
/// must be logged in (Settings → Connections). Used for in-app playback; the
/// download-to-library path in `provider-hires-provider` is separate.
/// What a provider load resolved to: a URL the player streams progressively
/// (playback starts after a small prefetch), or fully materialised bytes
/// (SoundCloud HLS, where the provider concatenates segments itself).
enum LoadedStream {
    Url(String),
    Bytes(Vec<u8>),
}

/// Hand a resolved stream to the audio engine. URL loads await the network
/// prefetch first and re-check staleness before committing, so a slow
/// prepare can never clobber a newer track. `None` = superseded, bail.
async fn start_stream(
    queue: &UseQueue,
    player: &Player,
    track: &Track,
    stream: LoadedStream,
    generation: u64,
    idx: usize,
    expected_uri: &str,
) -> Option<Result<(), String>> {
    match stream {
        LoadedStream::Url(url) => {
            let prepared = match Player::prepare_http(&url, Some(track.duration)).await {
                Ok(p) => p,
                Err(e) => return Some(Err(format!("stream open: {e}"))),
            };
            if !is_current_load(queue, generation, idx, expected_uri) {
                return None;
            }
            player.play_prepared(prepared);
            Some(Ok(()))
        }
        LoadedStream::Bytes(bytes) => {
            if !is_current_load(queue, generation, idx, expected_uri) {
                return None;
            }
            Some(player.play_bytes(bytes).map_err(|e| format!("decode: {e}")))
        }
    }
}

async fn load_qz(qz: &the hi-res providerProvider, track: &Track) -> Result<LoadedStream, String> {
    let stream = qz.resolve_stream(&track.uri).await.map_err(|e| {
        if matches!(e, provider_api::ProviderError::AuthRequired) {
            "Log in to the hi-res provider in Settings to stream this track.".to_string()
        } else {
            format!("the hi-res provider stream unavailable: {e}")
        }
    })?;
    match stream {
        StreamHandle::HttpStream { url, .. } => Ok(LoadedStream::Url(url)),
        StreamHandle::Bytes { data, .. } => Ok(LoadedStream::Bytes(data)),
        StreamHandle::InProcess { .. } => Err("Unexpected the hi-res provider stream type.".into()),
    }
}

async fn load_sc(sc: &SoundCloudProvider, track: &Track) -> Result<LoadedStream, String> {
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
        // Progressive: a single CDN URL — the player streams it directly.
        StreamHandle::HttpStream { url, .. } => Ok(LoadedStream::Url(url)),
        // HLS: the provider already resolved + concatenated all segments
        // because no single URL covers the audio.
        StreamHandle::Bytes { data, .. } => Ok(LoadedStream::Bytes(data)),
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
pub fn install(
    player: Player,
    sc: Arc<SoundCloudProvider>,
    sp: Arc<SpotifyProvider>,
    qz: Arc<the hi-res providerProvider>,
) {
    let entries = use_signal(Vec::<Track>::new);
    let current_index = use_signal(|| None::<usize>);
    let shuffle_enabled = use_signal(|| false);
    let repeat_mode = use_signal(|| RepeatMode::Off);
    let advance_state = use_signal(|| AdvanceState::Idle);
    let load_generation = use_signal(|| 0u64);
    let pre_shuffle = use_signal(|| None::<Vec<Track>>);
    let error = use_signal(|| None::<String>);
    let is_loading_track = use_signal(|| false);
    let radio_status = use_signal(|| RadioStatus::Idle);
    let gapless_prefetched = use_signal(|| None::<(u64, usize)>);
    let resume_hint = use_signal(|| None::<(String, Duration)>);

    let queue = UseQueue {
        entries,
        current_index,
        shuffle_enabled,
        repeat_mode,
        error,
        is_loading_track,
        advance_state,
        load_generation,
        pre_shuffle,
        radio_status,
        gapless_prefetched,
        resume_hint,
        sc,
        sp,
        qz,
        player: player.clone(),
        qz_swaps: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    use_context_provider({
        let queue = queue.clone();
        move || queue
    });

    // Restore the persisted queue before the first frame. Audio stays
    // stopped — the bottombar's play button already starts the queue at
    // current_index when nothing is loaded. Mid-track position is not
    // restored; resume starts the entry from the top.
    use_hook(move || {
        let Some(path) = config::AppConfig::queue_state_path() else {
            return;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return;
        };
        match serde_json::from_str::<PersistedQueue>(&raw) {
            Ok(saved) if !saved.entries.is_empty() => {
                let idx = saved
                    .current_index
                    .map(|i| i.min(saved.entries.len() - 1))
                    .unwrap_or(0);
                // Mid-track resume: arm a one-shot seek when the saved
                // position belongs to the entry we restored to. Positions
                // under 5 s aren't worth resuming into.
                if let Some(pos_path) = config::AppConfig::playback_position_path()
                    && let Ok(pos_raw) = std::fs::read_to_string(&pos_path)
                    && let Ok(saved_pos) = serde_json::from_str::<PersistedPosition>(&pos_raw)
                    && saved_pos.secs > 5
                    && saved.entries.get(idx).is_some_and(|t| t.uri.0 == saved_pos.uri)
                {
                    let mut hint = resume_hint;
                    hint.set(Some((saved_pos.uri, Duration::from_secs(saved_pos.secs))));
                }
                let mut entries_sig = entries;
                let mut current_sig = current_index;
                let mut shuffle_sig = shuffle_enabled;
                let mut repeat_sig = repeat_mode;
                entries_sig.set(saved.entries);
                current_sig.set(Some(idx));
                shuffle_sig.set(saved.shuffle);
                repeat_sig.set(saved.repeat);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "queue restore: parse failed; starting empty"),
        }
    });

    // Persist on every queue-shape change (entries/index/modes). Serialise
    // here (order fixed at enqueue time), write on the background persist
    // thread — this effect runs on the UI thread, and a synchronous disk
    // write per queue click was a visible input stall.
    let mut persist_prev_len = use_signal(|| usize::MAX);
    use_effect(move || {
        let state = PersistedQueue {
            entries: entries.read().clone(),
            current_index: *current_index.read(),
            shuffle: *shuffle_enabled.read(),
            repeat: *repeat_mode.read(),
        };
        // Diagnosis for "queue entries vanish": every shrink is logged with
        // both lengths so nira.log shows which mutation ate them.
        let prev = *persist_prev_len.peek();
        if state.entries.len() < prev && prev != usize::MAX {
            tracing::warn!(
                from = prev,
                to = state.entries.len(),
                index = ?state.current_index,
                "queue shrank"
            );
        }
        persist_prev_len.set(state.entries.len());
        let Some(path) = config::AppConfig::queue_state_path() else {
            return;
        };
        if let Err(e) = config::AppConfig::atomic_write_json_bg(path, &state) {
            tracing::warn!(error = %e, "queue persist failed");
        }
    });

    // Watcher — polls player.snapshot, drives the small state machine,
    // triggers auto-advance on natural track-end.
    use_hook(move || {
        let queue_for_watcher = queue.clone();
        let player_for_watcher = player.clone();
        spawn(async move {
            let queue = queue_for_watcher;
            let player = player_for_watcher;
            // Every 10th tick (~5 s) the in-track position is persisted for
            // the next boot's mid-track resume.
            let mut save_tick: u32 = 0;
            // Last observed playback position for dropout detection.
            let mut last_pos: Option<Duration> = None;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let snap = player.snapshot();
                let mut state = queue.advance_state;
                let current = *state.peek();

                // Dropout visibility: nominally playing but the position
                // didn't move across a 500 ms tick → the audio thread
                // produced nothing (underrun, blocked stream read, or a
                // system-level stall). Lands in nira.log with position and
                // track so reports can be correlated.
                if current == AdvanceState::Playing && snap.has_source && !snap.is_paused {
                    if let Some(prev) = last_pos
                        && snap.position == prev
                    {
                        let title = snap
                            .now_playing
                            .as_ref()
                            .map(|n| n.title.clone())
                            .unwrap_or_default();
                        tracing::warn!(
                            at_secs = snap.position.as_secs(),
                            %title,
                            "playback stalled: position frozen across a tick"
                        );
                    }
                    last_pos = Some(snap.position);
                } else {
                    last_pos = None;
                }

                save_tick = save_tick.wrapping_add(1);
                if save_tick % 10 == 0
                    && current == AdvanceState::Playing
                    && snap.has_source
                    && !snap.is_paused
                    && let Some(uri) = queue
                        .current_index
                        .peek()
                        .and_then(|i| queue.entries.peek().get(i).map(|t| t.uri.0.clone()))
                    && let Some(path) = config::AppConfig::playback_position_path()
                {
                    let pos = PersistedPosition {
                        uri,
                        secs: snap.position.as_secs(),
                    };
                    if let Err(e) = config::AppConfig::atomic_write_json_bg(path, &pos) {
                        tracing::debug!(error = %e, "position persist failed");
                    }
                }

                // Gapless hand-off happened inside the sink — walk the
                // index to the prefetched entry, no load, no state change.
                if player.take_gapless_advanced() {
                    if let Some((generation, idx)) = *queue.gapless_prefetched.peek()
                        && generation == *queue.load_generation.peek()
                    {
                        let mut current_sig = queue.current_index;
                        current_sig.set(Some(idx));
                        tracing::info!(idx, "queue: gapless hand-off committed");
                    }
                    let mut prefetched = queue.gapless_prefetched;
                    prefetched.set(None);
                }

                match current {
                    AdvanceState::Loading if snap.has_source => {
                        tracing::info!("queue: load succeeded, now Playing");
                        state.set(AdvanceState::Playing);
                        // One-shot mid-track resume from the previous
                        // session — only when this load IS the entry the
                        // saved position belongs to.
                        let hint = queue.resume_hint.peek().clone();
                        if let Some((uri, pos)) = hint {
                            let mut hint_sig = queue.resume_hint;
                            hint_sig.set(None);
                            let cur_uri = queue
                                .current_index
                                .peek()
                                .and_then(|i| queue.entries.peek().get(i).map(|t| t.uri.0.clone()));
                            if cur_uri.as_deref() == Some(uri.as_str()) {
                                tracing::info!(secs = pos.as_secs(), "resuming mid-track");
                                player.seek(pos);
                            }
                        }
                    }
                    AdvanceState::Playing if snap.has_source => {
                        maybe_prefetch_gapless(&queue, &snap);
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
                        TransportCmd::SeekFailed(msg) => {
                            let mut error = queue.error;
                            error.set(Some(msg));
                        }
                    }
                }
            });
        }
    });
}

pub fn use_queue() -> UseQueue {
    use_context::<UseQueue>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_api::{ArtistRef, ArtistUri, TrackUri};

    fn track(artist: &str, title: &str) -> Track {
        Track {
            uri: TrackUri(format!("soundcloud:track:{artist}-{title}")),
            provider: ProviderId::SoundCloud,
            title: title.into(),
            artists: vec![ArtistRef {
                uri: ArtistUri("soundcloud:user:1".into()),
                name: artist.into(),
            }],
            album: None,
            duration: Duration::from_secs(180),
            cover_url: None,
            mbid: None,
            added_at: None,
        }
    }

    fn result(artist: &str, title: &str) -> DiscoveryResult {
        DiscoveryResult {
            mbid: None,
            title: title.into(),
            artist: artist.into(),
            cover_url: None,
            spotify: None,
            soundcloud: Some(track(artist, title)),
            score: 1.0,
            rationale: String::new(),
        }
    }

    #[test]
    fn spread_artists_breaks_up_adjacent_runs() {
        let mut tracks = vec![
            track("A", "1"),
            track("A", "2"),
            track("B", "1"),
            track("A", "3"),
            track("C", "1"),
        ];
        spread_artists(&mut tracks);
        // Current track stays put.
        assert_eq!(tracks[0].title, "1");
        assert_eq!(tracks[0].artists[0].name, "A");
        for pair in tracks.windows(2) {
            assert_ne!(
                spread_key(&pair[0]),
                spread_key(&pair[1]),
                "adjacent same-artist pair survived: {:?}",
                tracks
                    .iter()
                    .map(|t| t.artists[0].name.clone())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn spread_artists_single_artist_queue_terminates_unchanged() {
        let mut tracks = vec![track("A", "1"), track("A", "2"), track("A", "3")];
        spread_artists(&mut tracks);
        let titles: Vec<_> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["1", "2", "3"]);
    }

    #[test]
    fn interleave_radio_blends_two_to_one_and_dedupes() {
        let main = vec![
            result("M", "Alpha"),
            result("M", "Beta"),
            result("M", "Gamma"),
        ];
        let profile = vec![
            result("P", "Delta"),
            result("M", "Alpha"),
            result("P", "Epsilon"),
        ];
        let blended = interleave_radio(main, profile);
        let names: Vec<String> = blended
            .iter()
            .map(|r| format!("{}-{}", r.artist, r.title))
            .collect();
        // 2 main : 1 profile, the duplicate Alpha is dropped.
        assert_eq!(
            names,
            vec!["M-Alpha", "M-Beta", "P-Delta", "M-Gamma", "P-Epsilon"]
        );
    }

    #[test]
    fn interleave_radio_collapses_same_song_across_sources() {
        // Same song, different uploader and upload noise — one copy survives.
        let main = vec![result("M", "Cool Song")];
        let profile = vec![result("P", "Cool Song (Official Audio)")];
        let blended = interleave_radio(main, profile);
        assert_eq!(blended.len(), 1);
        assert_eq!(blended[0].artist, "M");
    }
}
