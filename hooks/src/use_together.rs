//! Listen-together: publish what we play, or follow what the host plays.
//!
//! The `together` crate owns the connection and the clock offset. This is
//! where the policy lives, because the policy is really a statement about the
//! queue and it belongs next to the queue that enforces it.
//!
//! **Anchor, then verify.** Two machines playing the same file from local disk
//! drift apart only by their crystals, so the steady state is to line up once
//! and leave gaps inside the dead zone alone. A slower check catches startup
//! latency, stalls and scrubs without turning position sampling into a
//! high-frequency control loop.
//!
//! Gaps outside the dead zone do not decay: both players continue at the same
//! rate. They need one correction even when they are smaller than a scrub.

use std::time::Duration;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use provider_api::{
    AlbumRef, AlbumUri, ArtistRef, ArtistUri, ProviderId, Track, TrackUri,
};
use together::{RemoteNow, Role, Together, TogetherSnapshot};

use crate::queue::UseQueue;
use crate::use_local_library::UseLocalLibrary;

/// Below this, do nothing. Two peers in different rooms cannot perceive it,
/// and every boundary that carries a gap re-anchors anyway.
const DEAD_ZONE: Duration = Duration::from_millis(100);

/// Difference between consecutive host announcements that counts as a scrub.
const JUMP_THRESHOLD: Duration = Duration::from_millis(2000);

/// How often we compare. Deliberately slower than the queue watcher.
const COMPARE: Duration = Duration::from_secs(5);

/// Loop cadence while a session is up.
const TICK: Duration = Duration::from_millis(500);

/// Floor on how often a correction may be issued, however urgent it looks.
/// A seek is not free — on a progressive stream it can re-open the connection
/// — so correcting faster than this makes the drift it is chasing worse.
const MIN_CORRECT_GAP: Duration = Duration::from_secs(2);

/// Corrections that measurably fail to close the gap before we stop trying.
/// Without this the loop reissues a seek that is being refused (a dead audio
/// device, a source that cannot seek) twice a second, forever.
const FUTILE_LIMIT: u8 = 2;

/// Loop cadence while there is no session. This task lives for the whole
/// process and is idle for almost all of it, so the common case gets the
/// cheap cadence.
const IDLE_TICK: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub struct UseTogether {
    together: Signal<Together>,
    pub snapshot: Signal<TogetherSnapshot>,
    /// True while we are following a host. The queue watcher reads this and
    /// stops advancing on its own — otherwise both ends walk the queue at the
    /// same track end and race each other.
    pub following: Signal<bool>,
    /// "artist — title" of a host track we cannot play, so the UI can say so
    /// instead of leaving the guest staring at silence.
    pub unmatched: Signal<Option<String>>,
}

impl UseTogether {
    pub fn handle(&self) -> Together {
        self.together.peek().clone()
    }

    pub fn host(&self, name: String) {
        self.handle().host(name);
    }

    pub fn join(&self, code: String, name: String) {
        self.handle().join(code, name);
    }

    pub fn leave(&self) {
        self.handle().leave();
    }
}

pub fn use_together() -> UseTogether {
    use_context::<UseTogether>()
}

/// Build the wire payload from what the player is actually doing. `None` when
/// nothing is loaded, which reads on the guest side as "host stopped".
fn describe(queue: &UseQueue, player: &player::Player) -> Option<RemoteNow> {
    let idx = (*queue.current_index.peek())?;
    let entries = queue.entries.peek();
    // Deliberately the *queue entry*, not the player's now-playing: those can
    // differ, and the entry is the identity the guest has to resolve against
    // its own library.
    let track = entries.get(idx)?.clone();
    drop(entries);

    let snap = player.snapshot();
    let (pos, _at) = player.position_at();
    Some(RemoteNow {
        track_uri: track.uri.0.clone(),
        artist: track
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        title: track.title.clone(),
        album_title: track.album.as_ref().map(|album| album.title.clone()),
        cover_url: track.cover_url.clone(),
        duration_ns: track.duration.as_nanos() as u64,
        pos_ns: pos.as_nanos() as u64,
        // Stamped by Together::publish beside the position sample.
        at_ns: 0,
        playing: !snap.is_paused && snap.has_source,
        playback_id: snap.playback_id,
    })
}

/// The host's track as something [`crate::matching::find_strict_match`] can
/// compare — it keys on normalised (artist, title) plus duration, none of
/// which needs a real provider identity.
fn as_match_target(now: &RemoteNow) -> Track {
    Track {
        uri: TrackUri(now.track_uri.clone()),
        provider: ProviderId::Local,
        title: now.title.clone(),
        artists: vec![ArtistRef {
            uri: ArtistUri(String::new()),
            name: now.artist.clone(),
        }],
        album: now.album_title.as_ref().map(|title| AlbumRef {
            uri: AlbumUri(String::new()),
            title: title.clone(),
        }),
        duration: Duration::from_nanos(now.duration_ns),
        cover_url: now.cover_url.clone(),
        mbid: None,
        added_at: None,
    }
}

/// Where the host is *now*, extrapolated from its last announcement. `at_ns`
/// has already been translated onto our clock by the `together` crate.
fn expected_position(t: &Together, now: &RemoteNow) -> Duration {
    let elapsed = t.now_ns().saturating_sub(now.at_ns);
    let pos = if now.playing {
        now.pos_ns.saturating_add(elapsed)
    } else {
        now.pos_ns
    };
    Duration::from_nanos(pos)
}

pub fn install_together(queue: UseQueue, player: player::Player, local: UseLocalLibrary) {
    let together = use_signal(Together::new);
    let mut snapshot = use_signal(|| together.peek().snapshot());
    let mut following = use_signal(|| false);
    let mut unmatched = use_signal(|| None::<String>);

    use_context_provider(move || UseTogether {
        together,
        snapshot,
        following,
        unmatched,
    });

    // Root-scoped: the session outlives whatever page started it.
    spawn_forever(async move {
        // Tracks the playback we last lined ourselves up with, so a repeat of
        // the same track (same uri, new playback_id) still re-anchors.
        let mut anchored: Option<u64> = None;
        let mut since_compare = Duration::ZERO;
        // Set the moment we adopt a track, cleared once we have actually
        // lined up with the host. `play_track` always starts at zero, so
        // joining a host who is 90 seconds in needs one deliberate seek —
        // without it the guest would restart the track and only converge when
        // the periodic comparison eventually trips the resync threshold.
        let mut pending_align = false;
        // Last (position, timestamp, playing) the host announced, so a scrub
        // can be spotted the moment the next announcement contradicts it.
        let mut last_seen: Option<(u64, u64, bool)> = None;
        let mut last_correct_ns: u64 = 0;
        let mut last_err_ns: Option<u64> = None;
        let mut futile: u8 = 0;
        let mut host_stopped = false;

        let mut tick = IDLE_TICK;
        loop {
            tokio::time::sleep(tick).await;
            let t = together.peek().clone();
            let snap = t.snapshot();
            let role = snap.role;
            tick = if role == Role::Off { IDLE_TICK } else { TICK };

            if *snapshot.peek() != snap {
                snapshot.set(snap.clone());
            }
            let is_guest = role == Role::Guest;
            if *following.peek() != is_guest {
                following.set(is_guest);
            }
            // The watcher reads this every tick; setting it here keeps the
            // gate and the role from ever disagreeing.
            queue.set_follow_mode(is_guest);

            match role {
                Role::Host => {
                    t.publish(describe(&queue, &player));
                    anchored = None;
                    host_stopped = false;
                }
                Role::Off => {
                    anchored = None;
                    host_stopped = false;
                    if unmatched.peek().is_some() {
                        unmatched.set(None);
                    }
                }
                Role::Guest => {
                    if snap.stopped {
                        if !host_stopped {
                            queue.stop();
                            anchored = None;
                            pending_align = false;
                            last_seen = None;
                            host_stopped = true;
                        }
                        continue;
                    }
                    host_stopped = false;
                    let Some(target) = snap.target.clone() else {
                        continue;
                    };

                    // New playback on the host — load our own copy and line up
                    // from scratch. Position is not comparable across this.
                    if anchored != Some(target.playback_id) {
                        // Record the attempt whatever the outcome. Retrying a
                        // track we cannot play buys nothing and, before this,
                        // logged the same line twice a second for as long as
                        // the host kept playing it.
                        anchored = Some(target.playback_id);
                        since_compare = Duration::ZERO;
                        last_seen = None;
                        last_err_ns = None;
                        futile = 0;
                        match adopt(&queue, &local, &target) {
                            AdoptOutcome::Playing => {
                                pending_align = true;
                                if unmatched.peek().is_some() {
                                    unmatched.set(None);
                                }
                            }
                            AdoptOutcome::NotOurs => {
                                pending_align = false;
                                tracing::info!(
                                    title = %target.title,
                                    artist = %target.artist,
                                    uri = %target.track_uri,
                                    "together: host is on a local file we do not have"
                                );
                                unmatched.set(Some(format!(
                                    "{} — {}",
                                    target.artist, target.title
                                )));
                            }
                        }
                        continue;
                    }

                    // Land on the host's position as soon as the decoder is
                    // up. Until then there is nothing to seek.
                    if pending_align {
                        if player.snapshot().has_source {
                            let to = expected_position(&t, &target);
                            tracing::info!(at_ms = to.as_millis(), "together: aligning to host");
                            player.seek(to);
                            pending_align = false;
                            since_compare = Duration::ZERO;
                        }
                        continue;
                    }

                    // A scrub on the host is not drift and must not wait for
                    // the comparison timer.
                    let jumped = last_seen.is_some_and(|p| host_jumped(p, &target));
                    last_seen = Some((target.pos_ns, target.at_ns, target.playing));

                    since_compare += TICK;
                    if !jumped && since_compare < COMPARE {
                        continue;
                    }
                    // Give up rather than hammer. A correction that does not
                    // move us is being refused by something we cannot see from
                    // here, and repeating it every tick only floods the log and
                    // fights the queue.
                    if futile >= FUTILE_LIMIT {
                        continue;
                    }
                    let now_ns = t.now_ns();
                    if now_ns.saturating_sub(last_correct_ns)
                        < MIN_CORRECT_GAP.as_nanos() as u64
                    {
                        continue;
                    }
                    since_compare = Duration::ZERO;
                    last_correct_ns = now_ns;

                    let err = correct(&player, &t, &target);
                    // "Worked" means the gap shrank by a quarter or better.
                    match (last_err_ns, err) {
                        (Some(prev), Some(cur)) if cur >= prev - prev / 4 => {
                            futile += 1;
                            if futile >= FUTILE_LIMIT {
                                tracing::warn!(
                                    gap_ms = cur / 1_000_000,
                                    "together: correction is not taking effect, \
                                     leaving playback alone"
                                );
                            }
                        }
                        _ => futile = 0,
                    }
                    last_err_ns = err;
                }
            }
        }
    });
}

/// Start playing the host's track locally. Step 1 only handles the case where
/// we already own it; anything else is reported and skipped rather than
/// silently leaving the guest on the previous track.
fn adopt(queue: &UseQueue, local: &UseLocalLibrary, target: &RemoteNow) -> AdoptOutcome {
    // A streaming provider's URI means the same thing on both machines, so the
    // guest can just play it. This is the common case in practice — the host is
    // usually playing something neither of us has on disk — and requiring a
    // local copy for it was the wrong default.
    if let Some(provider) = shared_provider(&target.track_uri) {
        let mut t = as_match_target(target);
        t.provider = provider;
        queue.play_track(t);
        return AdoptOutcome::Playing;
    }

    // A `local:` URI is a path on the host's machine and means nothing here, so
    // fall back to finding our own copy of the same recording.
    let wanted = as_match_target(target);
    let library = local.tracks.peek();
    let Some(found) = crate::matching::find_strict_match(&wanted, &library).cloned() else {
        return AdoptOutcome::NotOurs;
    };
    drop(library);
    queue.play_track(found);
    AdoptOutcome::Playing
}

#[derive(PartialEq)]
enum AdoptOutcome {
    Playing,
    /// The host is on a local file we do not have. Transferring it is a
    /// separate feature; until then the guest sits this track out.
    NotOurs,
}

/// Providers whose URIs address the same track from any machine.
///
/// Deliberately not exhaustive over `ProviderId`: a provider the guest is not
/// signed in to would fail at load time with a less useful error than simply
/// falling through to the local-library match.
fn shared_provider(uri: &str) -> Option<ProviderId> {
    if uri.starts_with("soundcloud:") {
        Some(ProviderId::SoundCloud)
    } else if uri.starts_with("spotify:") {
        Some(ProviderId::Spotify)
    } else {
        None
    }
}

fn needs_correction(mine: Duration, expected: Duration) -> bool {
    mine.abs_diff(expected) > DEAD_ZONE
}

/// Compare and, only if the gap is audible, correct.
fn correct(player: &player::Player, t: &Together, target: &RemoteNow) -> Option<u64> {
    let snap = player.snapshot();
    if !snap.has_source {
        return None;
    }
    let expected = expected_position(t, target);
    let (mine, _) = player.position_at();

    // Play/pause first: correcting a position while the transports disagree
    // would measure a gap that is about to close on its own.
    let we_are_playing = !snap.is_paused;
    if target.playing != we_are_playing {
        if target.playing {
            player.resume();
        } else {
            player.pause();
        }
        return None;
    }

    let gap = mine.abs_diff(expected);
    if needs_correction(mine, expected) {
        if mine < expected {
            tracing::info!(behind_ms = gap.as_millis(), "together: resyncing forward");
        } else {
            tracing::info!(ahead_ms = gap.as_millis(), "together: resyncing back");
        }
        player.seek(expected);
    }
    Some(gap.as_nanos() as u64)
}

/// Did the host's own timeline jump between two announcements?
///
/// Without this the guest only notices a scrub when the comparison timer next
/// comes round, so up to `COMPARE` plus a heartbeat of audibly wrong audio.
/// Comparing the host's new position against what its previous one predicted
/// spots the discontinuity on the very next message.
fn host_jumped(prev: (u64, u64, bool), now: &RemoteNow) -> bool {
    let (prev_pos, prev_at, prev_playing) = prev;
    // A transport change moves the position for legitimate reasons.
    if prev_playing != now.playing {
        return true;
    }
    // A paused host's position should not move at all, so no time is added and
    // any change is a scrub. Treating "still paused" as a jump — which this
    // did — made every single tick a correction for as long as the host stayed
    // paused.
    let elapsed = if now.playing {
        now.at_ns.saturating_sub(prev_at)
    } else {
        0
    };
    let predicted = prev_pos.saturating_add(elapsed);
    now.pos_ns.abs_diff(predicted) > JUMP_THRESHOLD.as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(pos_secs: u64, at_secs: u64, playing: bool) -> RemoteNow {
        RemoteNow {
            track_uri: "soundcloud:track:1".into(),
            artist: "a".into(),
            title: "t".into(),
            album_title: None,
            cover_url: None,
            duration_ns: Duration::from_secs(300).as_nanos() as u64,
            pos_ns: Duration::from_secs(pos_secs).as_nanos() as u64,
            at_ns: Duration::from_secs(at_secs).as_nanos() as u64,
            playing,
            playback_id: 1,
        }
    }

    /// Ordinary playback: the position advanced by exactly the elapsed time.
    /// Reporting this as a jump would make every heartbeat force a correction.
    #[test]
    fn steady_playback_is_not_a_jump() {
        assert!(!host_jumped((0, 0, true), &now(10, 10, true)));
        assert!(!host_jumped(
            (Duration::from_secs(60).as_nanos() as u64, 0, true),
            &now(62, 2, true)
        ));
    }

    /// Small clock and sampling noise must not read as a scrub either — the
    /// whole point of the dead zone is that we ignore it.
    #[test]
    fn sampling_noise_is_not_a_jump() {
        let prev = (Duration::from_secs(30).as_nanos() as u64, 0, true);
        let mut n = now(32, 2, true);
        n.pos_ns += Duration::from_millis(300).as_nanos() as u64;
        assert!(!host_jumped(prev, &n));
    }

    /// The case that broke: the host scrubs and the guest must find out now,
    /// not when the comparison timer next fires.
    #[test]
    fn a_scrub_in_either_direction_is_a_jump() {
        let prev = (Duration::from_secs(60).as_nanos() as u64, 0, true);
        assert!(host_jumped(prev, &now(120, 2, true)), "forward scrub missed");
        assert!(host_jumped(prev, &now(5, 2, true)), "backward scrub missed");
    }

    /// Transport changes move the position for legitimate reasons; re-checking
    /// is cheap and being wrong here is not.
    #[test]
    fn transport_changes_force_a_recheck() {
        assert!(host_jumped((0, 0, true), &now(10, 10, false)));
    }

    /// The regression that made every tick a correction: a host that simply
    /// stays paused is not moving and must not read as a jump.
    #[test]
    fn a_host_that_stays_paused_is_not_a_jump() {
        let prev = (Duration::from_secs(30).as_nanos() as u64, 0, false);
        assert!(!host_jumped(prev, &now(30, 10, false)));
        assert!(!host_jumped(prev, &now(30, 60, false)));
    }

    /// But scrubbing while paused still is one.
    #[test]
    fn scrubbing_while_paused_is_a_jump() {
        let prev = (Duration::from_secs(30).as_nanos() as u64, 0, false);
        assert!(host_jumped(prev, &now(90, 10, false)));
    }

    #[test]
    fn audible_gap_requires_correction_in_either_direction() {
        let expected = Duration::from_secs(30);
        assert!(needs_correction(
            expected - Duration::from_millis(500),
            expected
        ));
        assert!(needs_correction(
            expected + Duration::from_millis(500),
            expected
        ));
        assert!(!needs_correction(
            expected - Duration::from_millis(50),
            expected
        ));
    }
    #[test]
    fn transferred_track_keeps_visual_metadata() {
        let mut remote = now(30, 10, true);
        remote.album_title = Some("Geogaddi".into());
        remote.cover_url = Some("https://img.example/geogaddi.jpg".into());

        let track = as_match_target(&remote);
        assert_eq!(
            track.album.as_ref().map(|album| album.title.as_str()),
            Some("Geogaddi")
        );
        assert_eq!(
            track.cover_url.as_deref(),
            Some("https://img.example/geogaddi.jpg")
        );
    }
}
