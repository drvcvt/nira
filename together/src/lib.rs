//! Listen together — peer-to-peer synchronised playback.
//!
//! One host, N guests. The host announces what it is playing and when, on its
//! own monotonic clock; guests translate that through a measured clock offset
//! ([`clock::ClockSync`]) and line their own playback up with it.
//!
//! This crate is **transport and clock only**. It never touches `Player` or
//! the queue — it publishes what the host is doing and exposes what the host
//! said, and `hooks` does the applying. That keeps the dependency edge
//! one-way and means the sync policy lives next to the queue that enforces it.
//!
//! Transport is [iroh]: QUIC between endpoints identified by their ed25519
//! public key, so every connection is end-to-end encrypted and mutually
//! authenticated with no certificate handling on our side. Peers find each
//! other through a relay, then hole-punch to a direct link where the network
//! allows it; where it does not, traffic keeps flowing over the relay, still
//! encrypted end to end.
//!
//! [iroh]: https://docs.rs/iroh

pub mod clock;
mod net;

pub use net::{decode_ticket, encode_ticket};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Protocol identifier. Bump the suffix on any wire-incompatible change —
/// iroh refuses a connection whose ALPN it does not know, which turns a
/// version mismatch into a clean "can't connect" instead of a garbled decode.
pub const ALPN: &[u8] = b"nira/together/1";

/// Heartbeat interval. Also the probe interval — every heartbeat carries a
/// fresh clock probe, so link quality tracks the same cadence as state.
const BEAT: Duration = Duration::from_secs(2);

/// What the host is playing, timestamped on the host's clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteNow {
    /// Provider URI as it sits in the host's queue. Guests cannot resolve a
    /// `local:track:<path>` from another machine, so the fields below are what
    /// matching actually keys on; the URI is carried for logging and for the
    /// case where both peers share a provider.
    pub track_uri: String,
    pub artist: String,
    pub title: String,
    pub duration_ns: u64,
    /// Playback position at `at_ns`.
    pub pos_ns: u64,
    /// Host's monotonic clock when `pos_ns` was sampled.
    pub at_ns: u64,
    pub playing: bool,
    /// Monotonic per-playback id. A change means "new playback" — resync hard
    /// rather than nudging, since the position is not comparable across it.
    pub playback_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Msg {
    Hello { name: String },
    Ping { t1: u64 },
    /// `t1` echoed so the requester can pair it up without keeping state.
    Pong { t1: u64, t2: u64 },
    Now(Box<RemoteNow>),
    Bye,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Off,
    Host,
    Guest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TogetherSnapshot {
    pub role: Role,
    /// Connected peer names. Host side lists guests; guest side lists the host.
    pub peers: Vec<String>,
    /// Share string for the host's session — `None` unless hosting.
    pub ticket: Option<String>,
    pub status: String,
    /// Round-trip time of the probe the current clock offset came from.
    pub rtt_ms: Option<u64>,
    /// What the host says is playing, already translated onto *our* clock:
    /// `at_ns` is comparable with [`Together::now_ns`]. `None` on the host.
    pub target: Option<RemoteNow>,
}

struct Inner {
    epoch: Instant,
    role: RwLock<Role>,
    status: RwLock<String>,
    peers: RwLock<HashMap<u64, String>>,
    ticket: RwLock<Option<String>>,
    /// Host side: the latest state to broadcast. Guest side: unused.
    publish: RwLock<Option<RemoteNow>>,
    /// Guest side: the host's latest state, translated onto our clock.
    target: RwLock<Option<RemoteNow>>,
    sync: Mutex<clock::ClockSync>,
}

/// Handle to the listen-together session. Cheap to clone.
#[derive(Clone)]
pub struct Together {
    inner: Arc<Inner>,
}

impl Together {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                epoch: Instant::now(),
                role: RwLock::new(Role::Off),
                status: RwLock::new(String::new()),
                peers: RwLock::new(HashMap::new()),
                ticket: RwLock::new(None),
                publish: RwLock::new(None),
                target: RwLock::new(None),
                sync: Mutex::new(clock::ClockSync::new()),
            }),
        }
    }

    /// Our monotonic clock in nanoseconds. Not wall time and not comparable
    /// with a peer's raw value — that is what the clock offset is for.
    pub fn now_ns(&self) -> u64 {
        self.inner.epoch.elapsed().as_nanos() as u64
    }

    /// Host: hand the sync loop the current playback state. Called from the
    /// queue watcher on every state change and on its regular tick; cheap
    /// enough to call at the watcher's cadence.
    pub fn publish(&self, mut now: Option<RemoteNow>) {
        if *self.inner.role.read().unwrap_or_else(|p| p.into_inner()) != Role::Host {
            return;
        }
        if let Some(now) = &mut now {
            now.at_ns = self.now_ns();
        }
        *self.inner.publish.write().unwrap_or_else(|p| p.into_inner()) = now;
    }

    pub fn snapshot(&self) -> TogetherSnapshot {
        let i = &self.inner;
        let rd = |l: &RwLock<String>| l.read().unwrap_or_else(|p| p.into_inner()).clone();
        TogetherSnapshot {
            role: *i.role.read().unwrap_or_else(|p| p.into_inner()),
            peers: i
                .peers
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .values()
                .cloned()
                .collect(),
            ticket: i.ticket.read().unwrap_or_else(|p| p.into_inner()).clone(),
            status: rd(&i.status),
            rtt_ms: i
                .sync
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .rtt_ns()
                .map(|ns| ns / 1_000_000),
            target: i.target.read().unwrap_or_else(|p| p.into_inner()).clone(),
        }
    }

    fn set_status(&self, s: impl Into<String>) {
        *self.inner.status.write().unwrap_or_else(|p| p.into_inner()) = s.into();
    }
}

impl Default for Together {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_stamps_when_the_position_was_sampled() {
        let together = Together::new();
        *together
            .inner
            .role
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Role::Host;
        std::thread::sleep(Duration::from_millis(1));

        together.publish(Some(RemoteNow {
            track_uri: "local:track:/music/a.flac".into(),
            artist: "Boards of Canada".into(),
            title: "Roygbiv".into(),
            duration_ns: 151_000_000_000,
            pos_ns: 42_000_000_000,
            at_ns: 0,
            playing: true,
            playback_id: 7,
        }));

        let at_ns = together
            .inner
            .publish
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .unwrap()
            .at_ns;
        assert!(at_ns > 0);
        assert!(at_ns <= together.now_ns());
    }
}
