//! Local liked-songs store.
//!
//! Cross-provider: any Spotify or SoundCloud Track can be liked. State is
//! a Signal so UI reacts to toggles instantly; persistence happens via a
//! background atomic-write so the audio thread is never blocked by disk
//! I/O. The file lives in the user config dir (not cache) — losing the
//! list to a cache-clear would be too painful.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use config::{AppConfig, JsonLoad, load_json};
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
    notices: crate::UseDownloads,
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
        crate::report_persist(
            AppConfig::atomic_write_json_confirmed_bg(path, &items),
            self.notices,
            "Likes",
        );
    }
}

fn load_likes(path: Option<PathBuf>) -> (Vec<LikedTrack>, Option<PathBuf>) {
    let Some(path) = path else {
        return (Vec::new(), None);
    };
    match load_json(&path) {
        JsonLoad::Loaded(items) => (items, Some(path)),
        JsonLoad::Missing | JsonLoad::Quarantined { .. } => (Vec::new(), Some(path)),
        JsonLoad::Blocked { .. } => (Vec::new(), None),
    }
}

/// Install the global signal. Invalid bytes are preserved before recovery;
/// an unreadable original disables writes rather than risking replacement.
pub fn install_likes() {
    let (initial, path) = load_likes(AppConfig::likes_path());
    let items = use_signal(|| initial);
    let path_sig = use_signal(|| path);
    let notices = crate::use_downloads();
    use_context_provider(move || UseLikes {
        items,
        path: path_sig,
        notices,
    });
}

pub fn use_likes() -> UseLikes {
    use_context::<UseLikes>()
}

#[cfg(test)]
mod tests {
    use super::{LikedTrack, load_likes};

    #[test]
    fn saved_track_from_retired_provider_still_loads() {
        let json = r#"[{
            "track": {
                "uri": "retired:track:1",
                "provider": "RetiredProvider",
                "title": "Legacy",
                "artists": [],
                "album": null,
                "duration": {"secs": 1, "nanos": 0},
                "cover_url": null,
                "mbid": null,
                "added_at": null
            },
            "liked_at": "2026-07-28T10:11:30Z"
        }]"#;

        let saved: Vec<LikedTrack> = serde_json::from_str(json).unwrap();

        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].track.provider.label(), "Unavailable");
    }

    #[test]
    fn blocked_likes_file_disables_persistence() {
        let path = std::env::temp_dir().join(format!(
            "nira-likes-blocked-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir(&path).unwrap();

        let (likes, writable_path) = load_likes(Some(path.clone()));

        assert!(likes.is_empty());
        assert!(writable_path.is_none());
        assert!(path.is_dir());
        std::fs::remove_dir(path).unwrap();
    }
}
