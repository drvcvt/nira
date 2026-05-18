//! MPRIS D-Bus server. Linux only — exposes nira to playerctl, KDE/GNOME
//! now-playing widgets, and the system-media-keys daemon.
//!
//! Design:
//! - `NiraMprisImpl` is the bridge between mpris-server's traits and our
//!   `Player`. It holds an `Arc<Player>` and a `Mutex<MprisState>` mirror
//!   of the last published metadata so we know what changed when the
//!   snapshot tick comes around.
//! - A background watcher (spawned by `start`) polls the player snapshot
//!   every 500 ms and pushes property-change signals to the dbus server
//!   when relevant fields drift. We only publish *changes* — properties
//!   that look the same as last tick stay silent so dbus traffic stays low.
//!
//! mpris-server runs its own async-io executor for the dbus connection
//! (zbus default). That lives alongside our tokio runtime without conflict —
//! the shared state is just `Arc<Mutex>`.

#![cfg(target_os = "linux")]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use hooks::Player;
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property,
    RootInterface, Server, Signal as MprisSignal, Time, TrackId, Volume,
    zbus::fdo,
};

/// Start the MPRIS server in the background. Failures are logged and the
/// rest of the app continues — a missing or broken D-Bus session shouldn't
/// stop the audio engine.
pub fn start(player: Player) {
    std::thread::Builder::new()
        .name("nira-mpris".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!(error = %e, "could not build MPRIS runtime");
                    return;
                }
            };
            rt.block_on(async move {
                if let Err(e) = run(player).await {
                    tracing::warn!(error = %e, "MPRIS server exited");
                }
            });
        })
        .ok();
}

async fn run(player: Player) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(StdMutex::new(MprisState::default()));
    let imp = NiraMprisImpl {
        player: player.clone(),
        state: state.clone(),
    };
    let server = Server::new("nira", imp).await?;
    tracing::info!("MPRIS server listening on {}", server.bus_name());

    // Snapshot watcher — emit PropertiesChanged when fields drift.
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let snap = player.snapshot();
        let mut changes: Vec<Property> = Vec::new();

        let playback_status = match (snap.has_source, snap.is_paused) {
            (false, _) => PlaybackStatus::Stopped,
            (true, true) => PlaybackStatus::Paused,
            (true, false) => PlaybackStatus::Playing,
        };
        let metadata = build_metadata(&snap);
        let volume: Volume = snap.volume as f64;
        let can_seek = snap.has_source;

        {
            let mut s = state.lock().unwrap();
            if s.playback_status != Some(playback_status) {
                s.playback_status = Some(playback_status);
                changes.push(Property::PlaybackStatus(playback_status));
            }
            if s.metadata.as_ref() != Some(&metadata) {
                s.metadata = Some(metadata.clone());
                changes.push(Property::Metadata(metadata));
            }
            if s.volume != Some(volume) {
                s.volume = Some(volume);
                changes.push(Property::Volume(volume));
            }
            if s.can_seek != Some(can_seek) {
                s.can_seek = Some(can_seek);
                changes.push(Property::CanSeek(can_seek));
            }
        }

        if !changes.is_empty() {
            let _ = server.properties_changed(changes).await;
        }
        // Always emit a Seeked-style position update so progress widgets
        // (e.g. KDE's panel) stay in sync. mpris-server has a Position
        // signal for this; without it clients poll position themselves.
        let pos_micros = snap.position.as_micros() as i64;
        let _ = server
            .emit(MprisSignal::Seeked {
                position: Time::from_micros(pos_micros),
            })
            .await;
    }
}

fn build_metadata(snap: &hooks::PlayerSnapshot) -> Metadata {
    let mut m = Metadata::new();
    m.set_trackid(Some(track_id_for(snap)));
    if let Some(np) = snap.now_playing.as_ref() {
        m.set_title(Some(np.title.clone()));
        if !np.artist.is_empty() {
            m.set_artist(Some(vec![np.artist.clone()]));
        }
        if let Some(cover) = np.cover_url.clone() {
            m.set_art_url(Some(cover));
        }
    }
    if let Some(d) = snap.duration {
        m.set_length(Some(Time::from_micros(d.as_micros() as i64)));
    }
    m
}

fn track_id_for(snap: &hooks::PlayerSnapshot) -> TrackId {
    let Some(np) = snap.now_playing.as_ref() else {
        return TrackId::NO_TRACK;
    };
    let mut hasher = DefaultHasher::new();
    np.provider.hash(&mut hasher);
    np.source_label.hash(&mut hasher);
    np.artist.hash(&mut hasher);
    np.title.hash(&mut hasher);
    let path = format!("/dev/nira/track/{:016x}", hasher.finish());
    TrackId::try_from(path).unwrap_or(TrackId::NO_TRACK)
}

fn duration_micros(duration: Duration) -> i128 {
    duration.as_micros().min(i128::MAX as u128) as i128
}

fn clamp_seek_micros(target: i128, duration: Option<Duration>) -> u64 {
    let upper = duration.map(duration_micros);
    let clamped = target.max(0);
    let clamped = upper.map(|u| clamped.min(u)).unwrap_or(clamped);
    clamped.min(u64::MAX as i128) as u64
}

fn seek_to_micros(player: &Player, micros: u64) {
    player.seek(Duration::from_micros(micros));
}

#[derive(Default)]
struct MprisState {
    playback_status: Option<PlaybackStatus>,
    metadata: Option<Metadata>,
    volume: Option<Volume>,
    can_seek: Option<bool>,
}

struct NiraMprisImpl {
    player: Player,
    state: Arc<StdMutex<MprisState>>,
}

impl RootInterface for NiraMprisImpl {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }
    async fn quit(&self) -> fdo::Result<()> {
        Ok(())
    }
    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn set_fullscreen(&self, _: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn identity(&self) -> fdo::Result<String> {
        Ok("nira".to_string())
    }
    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("nira".to_string())
    }
    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![])
    }
}

impl PlayerInterface for NiraMprisImpl {
    async fn next(&self) -> fdo::Result<()> {
        // Fire-and-forget into the transport bus. Queue install drained the
        // receiver on app boot; if for some reason it didn't (e.g. queue
        // panic before the bus consumer attached), the send is a silent
        // no-op rather than an error — MPRIS clients expect Next to
        // "succeed" even when there's nothing playable.
        self.player.request_next();
        Ok(())
    }
    async fn previous(&self) -> fdo::Result<()> {
        self.player.request_previous();
        Ok(())
    }
    async fn pause(&self) -> fdo::Result<()> {
        self.player.pause();
        Ok(())
    }
    async fn play_pause(&self) -> fdo::Result<()> {
        let snap = self.player.snapshot();
        if snap.is_paused {
            self.player.resume();
        } else {
            self.player.pause();
        }
        Ok(())
    }
    async fn stop(&self) -> fdo::Result<()> {
        self.player.stop();
        Ok(())
    }
    async fn play(&self) -> fdo::Result<()> {
        self.player.resume();
        Ok(())
    }
    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let snap = self.player.snapshot();
        if !snap.has_source {
            return Ok(());
        }
        let current = snap.position.as_micros().min(i128::MAX as u128) as i128;
        let target = current.saturating_add(offset.as_micros() as i128);
        if let Some(duration) = snap.duration
            && target > duration_micros(duration)
        {
            self.player.request_next();
            return Ok(());
        }
        seek_to_micros(&self.player, clamp_seek_micros(target, snap.duration));
        Ok(())
    }
    async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
        let snap = self.player.snapshot();
        if !snap.has_source || track_id != track_id_for(&snap) {
            return Ok(());
        }
        let target = position.as_micros() as i128;
        if let Some(duration) = snap.duration
            && target > duration_micros(duration)
        {
            return Ok(());
        }
        seek_to_micros(&self.player, clamp_seek_micros(target, snap.duration));
        Ok(())
    }
    async fn open_uri(&self, _: String) -> fdo::Result<()> {
        Ok(())
    }
    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        let snap = self.player.snapshot();
        Ok(match (snap.has_source, snap.is_paused) {
            (false, _) => PlaybackStatus::Stopped,
            (true, true) => PlaybackStatus::Paused,
            (true, false) => PlaybackStatus::Playing,
        })
    }
    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(LoopStatus::None)
    }
    async fn set_loop_status(&self, _: LoopStatus) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn set_rate(&self, _: PlaybackRate) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(false)
    }
    async fn set_shuffle(&self, _: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }
    async fn metadata(&self) -> fdo::Result<Metadata> {
        let snap = self.player.snapshot();
        Ok(build_metadata(&snap))
    }
    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.player.snapshot().volume as f64)
    }
    async fn set_volume(&self, volume: Volume) -> mpris_server::zbus::Result<()> {
        self.player.set_volume(volume as f32);
        if let Ok(mut s) = self.state.lock() {
            s.volume = Some(volume);
        }
        Ok(())
    }
    async fn position(&self) -> fdo::Result<Time> {
        let pos = self.player.snapshot().position;
        Ok(Time::from_micros(pos.as_micros() as i64))
    }
    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }
    async fn can_go_next(&self) -> fdo::Result<bool> {
        // Always advertise these — MPRIS clients (playerctl, KDE/GNOME
        // widgets) hide the buttons entirely when they're false. The
        // transport bus tolerates a Next/Previous arriving with nothing in
        // the queue: queue.next() returns early if there's no current_index.
        Ok(true)
    }
    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }
    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.player.snapshot().has_source)
    }
    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}
