//! Local liked-songs store.
//!
//! Cross-provider: any Spotify or SoundCloud Track can be liked. State is
//! a Signal so UI reacts to toggles instantly; persistence happens via a
//! background atomic-write so the audio thread is never blocked by disk
//! I/O. The file lives in the user config dir (not cache) — losing the
//! list to a cache-clear would be too painful.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use config::AppConfig;
use dioxus::prelude::*;
use provider_api::{Track, TrackUri};
use serde::{Deserialize, Serialize};

/// One saved entry. We persist the full Track so liked songs survive even
/// if the provider later removes them from search results / public APIs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LikedTrack {
    pub track: Track,
    pub liked_at: DateTime<Utc>,
}

#[derive(Clone, Copy)]
pub struct UseLikes {
    pub items: Signal<Vec<LikedTrack>>,
    pub path: Signal<Option<PathBuf>>,
}

impl UseLikes {
    pub fn is_liked(&self, uri: &TrackUri) -> bool {
        self.items.read().iter().any(|l| l.track.uri == *uri)
    }

    pub fn list(&self) -> Vec<LikedTrack> {
        self.items.read().clone()
    }

    pub fn count(&self) -> usize {
        self.items.read().len()
    }

    /// Add the track if it isn't liked yet; remove it if it is. Persisted
    /// asynchronously — UI updates immediately, disk catches up.
    pub fn toggle(&self, track: &Track) {
        let mut items = self.items;
        let mut current = items.peek().clone();
        if let Some(pos) = current.iter().position(|l| l.track.uri == track.uri) {
            current.remove(pos);
        } else {
            current.insert(
                0,
                LikedTrack {
                    track: track.clone(),
                    liked_at: Utc::now(),
                },
            );
        }
        items.set(current.clone());
        self.persist(current);
    }

    fn persist(&self, items: Vec<LikedTrack>) {
        let Some(path) = self.path.peek().clone() else {
            return;
        };
        // Detach from the Dioxus runtime; the writer is fully self-contained.
        std::thread::spawn(move || {
            if let Err(e) = AppConfig::atomic_write_json(&path, &items) {
                tracing::warn!("likes persist failed: {e}");
            }
        });
    }
}

/// Install the global signal. Loads from disk on first call (best-effort —
/// any read/parse error is logged and treated as "empty list"; we don't
/// want a corrupted file to crash the app on boot).
pub fn install_likes() {
    let path = AppConfig::likes_path();
    let initial: Vec<LikedTrack> = match &path {
        Some(p) if p.exists() => match std::fs::read_to_string(p) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("likes load: parse failed: {e}; starting empty");
                Vec::new()
            }),
            Err(e) => {
                tracing::warn!("likes load: read failed: {e}; starting empty");
                Vec::new()
            }
        },
        _ => Vec::new(),
    };
    let items = use_signal(|| initial);
    let path_sig = use_signal(|| path);
    use_context_provider(move || UseLikes {
        items,
        path: path_sig,
    });
}

pub fn use_likes() -> UseLikes {
    use_context::<UseLikes>()
}
