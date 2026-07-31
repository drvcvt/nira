use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{self, ActivityType, Assets, StatusDisplayType, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use player::{Player, PlayerSnapshot};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const SEEK_DRIFT: Duration = Duration::from_secs(2);
const UPDATE_WINDOW: Duration = Duration::from_secs(20);
const MAX_UPDATES_PER_WINDOW: usize = 5;
const APPLICATION_ID: &str = "1532715723041013830";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Presence {
    title: String,
    artist: String,
    album: Option<String>,
    cover_url: Option<String>,
    is_paused: bool,
    position: Duration,
    duration: Option<Duration>,
    playback_id: u64,
}

impl Presence {
    fn from_snapshot(snapshot: &PlayerSnapshot) -> Option<Self> {
        let now_playing = snapshot.now_playing.as_ref()?;
        snapshot.has_source.then(|| Self {
            title: now_playing.title.clone(),
            artist: now_playing.artist.clone(),
            album: now_playing.album.clone(),
            cover_url: now_playing
                .cover_url
                .as_ref()
                .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
                .cloned(),
            is_paused: snapshot.is_paused,
            position: snapshot.position,
            duration: snapshot.duration,
            playback_id: snapshot.playback_id,
        })
    }

    fn activity(&self, now_ms: i64) -> activity::Activity<'_> {
        let mut activity = activity::Activity::new()
            .activity_type(ActivityType::Listening)
            .status_display_type(StatusDisplayType::Details)
            .details(&self.title)
            .state(&self.artist);

        if let Some(cover_url) = &self.cover_url {
            let mut assets = Assets::new().large_image(cover_url);
            if let Some(album) = &self.album {
                assets = assets.large_text(album);
            }
            activity = activity.assets(assets);
        }

        if !self.is_paused
            && let Some(duration) = self.duration
        {
            let position_ms = millis(self.position.min(duration));
            let start_ms = now_ms.saturating_sub(position_ms);
            activity = activity.timestamps(
                Timestamps::new()
                    .start(start_ms)
                    .end(start_ms.saturating_add(millis(duration))),
            );
        }

        activity
    }
}

fn millis(duration: Duration) -> i64 {
    duration.as_millis().min(i64::MAX as u128) as i64
}

fn refresh_needed(previous: &Presence, elapsed: Duration, current: &Presence) -> bool {
    if previous.title != current.title
        || previous.artist != current.artist
        || previous.album != current.album
        || previous.cover_url != current.cover_url
        || previous.is_paused != current.is_paused
        || previous.duration != current.duration
        || previous.playback_id != current.playback_id
    {
        return true;
    }

    !current.is_paused
        && previous
            .position
            .saturating_add(elapsed)
            .abs_diff(current.position)
            > SEEK_DRIFT
}

fn update_allowed(updates: &mut VecDeque<Instant>, now: Instant) -> bool {
    while updates
        .front()
        .is_some_and(|sent_at| now.duration_since(*sent_at) >= UPDATE_WINDOW)
    {
        updates.pop_front();
    }
    updates.len() < MAX_UPDATES_PER_WINDOW
}

fn projected_presence(snapshot: &PlayerSnapshot, enabled: bool) -> Option<Presence> {
    enabled.then(|| Presence::from_snapshot(snapshot)).flatten()
}

pub fn start(player: Player, enabled: Arc<AtomicBool>) {
    if let Err(error) = std::thread::Builder::new()
        .name("nira-discord".into())
        .spawn(move || run(player, enabled, APPLICATION_ID))
    {
        tracing::warn!(%error, "could not start Discord presence thread");
    }
}

fn run(player: Player, enabled: Arc<AtomicBool>, application_id: &'static str) {
    let mut client = DiscordIpcClient::new(application_id);
    let mut connected = false;
    let mut last_attempt = Instant::now()
        .checked_sub(RETRY_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut last_sent: Option<(Presence, Instant)> = None;
    let mut cleared = false;
    let mut updates = VecDeque::new();

    loop {
        let now = Instant::now();
        let enabled = enabled.load(Ordering::Relaxed);
        if enabled && !connected && now.duration_since(last_attempt) >= RETRY_INTERVAL {
            last_attempt = now;
            if client.connect().is_ok() {
                connected = true;
                last_sent = None;
                cleared = false;
                tracing::debug!("connected Discord presence");
            }
        }

        if connected {
            let current = projected_presence(&player.snapshot(), enabled);
            let needs_set = current.as_ref().is_some_and(|presence| {
                last_sent.as_ref().is_none_or(|(previous, sent_at)| {
                    sent_at.elapsed() >= HEARTBEAT_INTERVAL
                        || refresh_needed(previous, sent_at.elapsed(), presence)
                })
            });
            let needs_clear = current.is_none() && !cleared;
            // Privacy-off clears immediately even if recent track changes used
            // the normal Discord update budget.
            let can_update = needs_clear && !enabled
                || (needs_set || needs_clear) && update_allowed(&mut updates, now);
            let result = match current {
                Some(presence) if needs_set && can_update => client
                    .set_activity(presence.activity(unix_ms()))
                    .and_then(|_| client.recv().map(|_| ()))
                    .map(|_| {
                        last_sent = Some((presence, now));
                        cleared = false;
                        updates.push_back(now);
                    }),
                None if needs_clear && can_update => client
                    .clear_activity()
                    .and_then(|_| client.recv().map(|_| ()))
                    .map(|_| {
                        last_sent = None;
                        cleared = true;
                        updates.push_back(now);
                    }),
                _ => Ok(()),
            };

            if let Err(error) = result {
                tracing::debug!(%error, "lost Discord presence connection");
                connected = false;
                last_sent = None;
                client = DiscordIpcClient::new(application_id);
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

fn unix_ms() -> i64 {
    millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}

#[cfg(test)]
fn presence_json(snapshot: &PlayerSnapshot, now_ms: i64) -> Option<serde_json::Value> {
    Presence::from_snapshot(snapshot)
        .and_then(|presence| serde_json::to_value(presence.activity(now_ms)).ok())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    use super::{Presence, presence_json, projected_presence, refresh_needed, update_allowed};
    use player::{Active, NowPlaying, PlayerSnapshot};

    fn snapshot(
        provider: &str,
        source_label: &str,
        track_uri: &str,
        is_paused: bool,
    ) -> PlayerSnapshot {
        PlayerSnapshot {
            is_paused,
            volume: 0.8,
            position: Duration::from_secs(40),
            duration: Some(Duration::from_secs(200)),
            has_source: true,
            now_playing: Some(NowPlaying {
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                cover_url: Some("https://example.com/cover.jpg".into()),
                source_label: source_label.into(),
                provider: provider.into(),
                track_uri: Some(track_uri.into()),
            }),
            playback_id: 7,
            active: Active::Spotify,
            transport_locked: false,
        }
    }

    #[test]
    fn activity_projection_is_provider_blind() {
        let first = presence_json(
            &snapshot("Provider A", "provider-a", "a:track:1", false),
            1_700_000_100_000,
        )
        .expect("first provider projects");
        let mut second_snapshot = snapshot("Provider B", "provider-b", "b:track:1", false);
        second_snapshot.active = Active::Rodio;
        let second =
            presence_json(&second_snapshot, 1_700_000_100_000).expect("second provider projects");

        assert_eq!(first, second);
        assert_eq!(first["type"], 2);
        assert_eq!(first["status_display_type"], 2);
        assert_eq!(first["details"], "Song");
        assert_eq!(first["state"], "Artist");
        assert_eq!(
            first["assets"]["large_image"],
            "https://example.com/cover.jpg"
        );
        assert_eq!(first["assets"]["large_text"], "Album");
    }

    #[test]
    fn paused_activity_has_no_timestamps() {
        let value = presence_json(
            &snapshot("Provider B", "provider-b", "b:track:1", true),
            1_700_000_100_000,
        )
        .expect("playing snapshot");

        assert!(value.get("timestamps").is_none());
    }

    #[test]
    fn disabled_presence_is_hidden() {
        assert!(
            projected_presence(
                &snapshot("Provider A", "provider-a", "a:track:1", false),
                false,
            )
            .is_none()
        );
    }

    #[test]
    fn steady_progress_does_not_refresh_presence() {
        let before =
            Presence::from_snapshot(&snapshot("Provider A", "provider-a", "a:track:1", false))
                .expect("playing snapshot");
        let mut later = before.clone();
        later.position += Duration::from_millis(500);

        assert!(!refresh_needed(&before, Duration::from_millis(500), &later));
    }

    #[test]
    fn seek_refreshes_presence_timestamps() {
        let before =
            Presence::from_snapshot(&snapshot("Provider A", "provider-a", "a:track:1", false))
                .expect("playing snapshot");
        let mut later = before.clone();
        later.position += Duration::from_secs(10);

        assert!(refresh_needed(&before, Duration::from_millis(500), &later));
    }

    #[test]
    fn discord_update_limit_is_respected() {
        let start = Instant::now();
        let mut updates = VecDeque::new();
        for second in 0..5 {
            let now = start + Duration::from_secs(second);
            assert!(update_allowed(&mut updates, now));
            updates.push_back(now);
        }

        assert!(!update_allowed(
            &mut updates,
            start + Duration::from_secs(19)
        ));
        assert!(update_allowed(
            &mut updates,
            start + Duration::from_secs(20)
        ));
    }
}
