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

#[derive(Debug, Clone)]
pub struct PlaylistImport {
    pub source_id: String,
    pub name: String,
    pub tracks: Vec<Track>,
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

    /// True when a source playlist has already been imported.
    pub fn has_import(&self, source: &str, source_id: &str) -> bool {
        let id = external_playlist_id(source, source_id);
        self.items.read().iter().any(|playlist| playlist.id == id)
    }

    /// Import source playlists once without overwriting later local edits.
    /// Returns the number of newly added playlists.
    pub fn import_external(&self, source: &str, playlists: Vec<PlaylistImport>) -> usize {
        let mut added = 0;
        self.mutate(|items| added = merge_external_playlists(items, source, playlists));
        added
    }

    /// Compatibility for the current Library button. Removed with the
    /// provider-neutral import dialog.
    pub fn import_spotify(&self, playlists: Vec<(String, String, Vec<Track>)>) -> usize {
        self.import_external(
            "spotify",
            playlists
                .into_iter()
                .map(|(source_id, name, tracks)| PlaylistImport {
                    source_id,
                    name,
                    tracks,
                })
                .collect(),
        )
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

fn external_playlist_id(source: &str, source_id: &str) -> String {
    format!("{}-{}", source.trim().to_ascii_lowercase(), source_id.trim())
}

fn merge_external_playlists(
    items: &mut Vec<Playlist>,
    source: &str,
    playlists: Vec<PlaylistImport>,
) -> usize {
    let now = Utc::now();
    let mut additions = Vec::new();
    for playlist in playlists {
        let id = external_playlist_id(source, &playlist.source_id);
        if items.iter().chain(&additions).any(|p| p.id == id) {
            continue;
        }
        additions.push(Playlist {
            id,
            name: playlist.name,
            tracks: playlist.tracks,
            albums: Vec::new(),
            created_at: now,
            updated_at: now,
        });
    }
    let added = additions.len();
    additions.append(items);
    *items = additions;
    added
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_imports_are_source_scoped_and_non_destructive() {
        let now = Utc::now();
        let mut items = vec![Playlist {
            id: "spotify-kept".into(),
            name: "My local rename".into(),
            tracks: Vec::new(),
            albums: Vec::new(),
            created_at: now,
            updated_at: now,
        }];

        let added = merge_external_playlists(
            &mut items,
            "spotify",
            vec![
                PlaylistImport {
                    source_id: "kept".into(),
                    name: "Remote rename".into(),
                    tracks: Vec::new(),
                },
                PlaylistImport {
                    source_id: "new".into(),
                    name: "New playlist".into(),
                    tracks: Vec::new(),
                },
            ],
        );

        assert_eq!(added, 1);
        assert_eq!(items[0].id, "spotify-new");
        assert_eq!(items[1].name, "My local rename");
    }

    #[test]
    fn equal_provider_ids_do_not_collide_across_sources() {
        let mut items = Vec::new();
        let spotify = PlaylistImport {
            source_id: "42".into(),
            name: "Spotify".into(),
            tracks: Vec::new(),
        };
        let soundcloud = PlaylistImport {
            source_id: "42".into(),
            name: "SoundCloud".into(),
            tracks: Vec::new(),
        };

        assert_eq!(
            merge_external_playlists(&mut items, "spotify", vec![spotify]),
            1
        );
        assert_eq!(
            merge_external_playlists(&mut items, "soundcloud", vec![soundcloud]),
            1
        );
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|playlist| playlist.id == "spotify-42"));
        assert!(
            items
                .iter()
                .any(|playlist| playlist.id == "soundcloud-42")
        );
    }

    #[test]
    fn removed_import_can_be_imported_again() {
        let mut items = Vec::new();
        let imported = PlaylistImport {
            source_id: "road-trip".into(),
            name: "Road trip".into(),
            tracks: Vec::new(),
        };

        assert_eq!(
            merge_external_playlists(&mut items, "spotify", vec![imported.clone()]),
            1
        );
        items.retain(|playlist| playlist.id != "spotify-road-trip");
        assert_eq!(
            merge_external_playlists(&mut items, "spotify", vec![imported]),
            1
        );
    }
}
