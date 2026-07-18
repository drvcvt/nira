//! Local playlists — cross-provider, hand-curated.
//!
//! Same shape as the likes store: a Signal for instant UI reaction, JSON in
//! the config dir (cache wipes must not eat hand-built lists), atomic writes
//! off-thread. Full `Track`s are persisted so a playlist survives a provider
//! dropping the track from search.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use config::AppConfig;
use dioxus::prelude::*;
use provider_api::{Track, TrackUri};
use serde::{Deserialize, Serialize};

/// A whole album embedded in a playlist — rendered as its own widget with
/// an expandable track list, not exploded into loose rows. Tracks are
/// snapshotted at add time (same survival argument as loose tracks).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlaylistAlbum {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub cover_url: Option<String>,
    pub tracks: Vec<Track>,
    pub added_at: DateTime<Utc>,
}

impl PlaylistAlbum {
    /// Snapshot an album right-click payload for embedding.
    pub fn from_ctx(a: &crate::use_ctx_menu::AlbumCtx) -> Self {
        Self {
            uri: a.uri.clone(),
            title: a.title.clone(),
            artist: a.artist.clone(),
            cover_url: a.cover_url.clone(),
            tracks: a.tracks.clone(),
            added_at: Utc::now(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub tracks: Vec<Track>,
    /// Album widgets. `default` keeps pre-albums playlists.json loading.
    #[serde(default)]
    pub albums: Vec<PlaylistAlbum>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Playlist {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.albums.is_empty()
    }

    /// Loose tracks followed by every album's tracks, in display order —
    /// what Play/Shuffle on the playlist header feed the queue.
    pub fn all_tracks(&self) -> Vec<Track> {
        let mut out = self.tracks.clone();
        for a in &self.albums {
            out.extend(a.tracks.iter().cloned());
        }
        out
    }
}

#[derive(Clone, Copy)]
pub struct UsePlaylists {
    pub items: Signal<Vec<Playlist>>,
    path: Signal<Option<PathBuf>>,
}

impl UsePlaylists {
    pub fn list(&self) -> Vec<Playlist> {
        self.items.read().clone()
    }

    pub fn count(&self) -> usize {
        self.items.read().len()
    }

    pub fn get(&self, id: &str) -> Option<Playlist> {
        self.items.read().iter().find(|p| p.id == id).cloned()
    }

    pub fn contains(&self, id: &str, uri: &TrackUri) -> bool {
        self.items
            .read()
            .iter()
            .find(|p| p.id == id)
            .is_some_and(|p| p.tracks.iter().any(|t| t.uri == *uri))
    }

    /// Create an empty playlist and return its id. Blank names fall back to
    /// "New Playlist"; duplicates are allowed (ids disambiguate).
    pub fn create(&self, name: &str) -> String {
        let name = name.trim();
        let name = if name.is_empty() { "New Playlist" } else { name };
        let now = Utc::now();
        let id = format!("pl-{}", now.timestamp_millis());
        let pl = Playlist {
            id: id.clone(),
            name: name.to_string(),
            tracks: Vec::new(),
            albums: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        self.mutate(|items| items.insert(0, pl));
        id
    }

    pub fn delete(&self, id: &str) {
        self.mutate(|items| items.retain(|p| p.id != id));
    }

    pub fn rename(&self, id: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.mutate(|items| {
            if let Some(p) = items.iter_mut().find(|p| p.id == id) {
                p.name = name.to_string();
                p.updated_at = Utc::now();
            }
        });
    }

    /// Append a track; returns false when it was already in the list.
    pub fn add_track(&self, id: &str, track: &Track) -> bool {
        let mut added = false;
        self.mutate(|items| {
            if let Some(p) = items.iter_mut().find(|p| p.id == id)
                && !p.tracks.iter().any(|t| t.uri == track.uri)
            {
                p.tracks.push(track.clone());
                p.updated_at = Utc::now();
                added = true;
            }
        });
        added
    }

    /// Swap a loose track with its neighbour (delta ±1).
    pub fn move_track(&self, id: &str, from: usize, delta: isize) {
        self.mutate(|items| {
            if let Some(p) = items.iter_mut().find(|p| p.id == id) {
                let to = from as isize + delta;
                if from < p.tracks.len() && to >= 0 && (to as usize) < p.tracks.len() {
                    p.tracks.swap(from, to as usize);
                    p.updated_at = Utc::now();
                }
            }
        });
    }

    /// Swap an album widget with its neighbour (delta ±1).
    pub fn move_album(&self, id: &str, from: usize, delta: isize) {
        self.mutate(|items| {
            if let Some(p) = items.iter_mut().find(|p| p.id == id) {
                let to = from as isize + delta;
                if from < p.albums.len() && to >= 0 && (to as usize) < p.albums.len() {
                    p.albums.swap(from, to as usize);
                    p.updated_at = Utc::now();
                }
            }
        });
    }

    pub fn contains_album(&self, id: &str, album_uri: &str) -> bool {
        self.items
            .read()
            .iter()
            .find(|p| p.id == id)
            .is_some_and(|p| p.albums.iter().any(|a| a.uri == album_uri))
    }

    /// Append an album widget; returns false when it's already in the list.
    pub fn add_album(&self, id: &str, album: &PlaylistAlbum) -> bool {
        let mut added = false;
        self.mutate(|items| {
            if let Some(p) = items.iter_mut().find(|p| p.id == id)
                && !p.albums.iter().any(|a| a.uri == album.uri)
            {
                p.albums.push(album.clone());
                p.updated_at = Utc::now();
                added = true;
            }
        });
        added
    }

    pub fn remove_album(&self, id: &str, album_uri: &str) {
        self.mutate(|items| {
            if let Some(p) = items.iter_mut().find(|p| p.id == id) {
                let before = p.albums.len();
                p.albums.retain(|a| a.uri != album_uri);
                if p.albums.len() != before {
                    p.updated_at = Utc::now();
                }
            }
        });
    }

    pub fn remove_track(&self, id: &str, uri: &TrackUri) {
        self.mutate(|items| {
            if let Some(p) = items.iter_mut().find(|p| p.id == id) {
                let before = p.tracks.len();
                p.tracks.retain(|t| t.uri != *uri);
                if p.tracks.len() != before {
                    p.updated_at = Utc::now();
                }
            }
        });
    }

    /// Clone-mutate-set-persist. UI updates immediately, disk catches up.
    fn mutate(&self, f: impl FnOnce(&mut Vec<Playlist>)) {
        let mut items = self.items;
        let mut current = items.peek().clone();
        f(&mut current);
        items.set(current.clone());
        let Some(path) = self.path.peek().clone() else {
            return;
        };
        if let Err(e) = AppConfig::atomic_write_json_bg(path, &current) {
            tracing::warn!("playlists persist failed: {e}");
        }
    }
}

/// Install the global signal. Loads from disk on first call — best-effort,
/// a corrupted file logs and starts empty rather than crashing boot.
pub fn install_playlists() {
    let path = AppConfig::playlists_path();
    let initial: Vec<Playlist> = match &path {
        Some(p) if p.exists() => match std::fs::read_to_string(p) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("playlists load: parse failed: {e}; starting empty");
                Vec::new()
            }),
            Err(e) => {
                tracing::warn!("playlists load: read failed: {e}; starting empty");
                Vec::new()
            }
        },
        _ => Vec::new(),
    };
    let items = use_signal(|| initial);
    let path_sig = use_signal(|| path);
    use_context_provider(move || UsePlaylists {
        items,
        path: path_sig,
    });
}

pub fn use_playlists() -> UsePlaylists {
    use_context::<UsePlaylists>()
}
