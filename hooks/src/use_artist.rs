//! Artist detail view — banner data + tabs (Top Tracks / Albums / Singles /
//! Related). The data lives on a single provider for v1; cross-provider
//! augmentation lands in a follow-up.
//!
//! Selecting a provider: the URI's prefix (`spotify:artist:…` /
//! `soundcloud:user:…`) decides which provider we query. Each tab fetches
//! lazily — top tracks come back with the initial banner load, albums and
//! related are pulled on first tab access.

use std::sync::Arc;

use dioxus::prelude::*;
use provider_api::{
    AlbumBrief, AlbumType, Artist, ArtistUri, Provider, ProviderId, RelatedArtist, Track,
};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

/// Banner + Top Tracks load together; the rest of the view fetches lazily.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtistView {
    pub artist: Artist,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<AlbumBrief>,
    /// Set once the albums tab has been opened and the fetch resolved.
    /// `None` means "not asked yet"; `Some(empty)` means "asked, got
    /// nothing back" — UI uses the distinction to show a skeleton vs empty.
    pub related: Option<Vec<RelatedArtist>>,
}

#[derive(Clone, Copy)]
pub struct UseArtist {
    pub view: Signal<Option<ArtistView>>,
    pub is_loading: Signal<bool>,
    pub is_loading_related: Signal<bool>,
    pub error: Signal<Option<String>>,
    sc: Signal<Option<Arc<SoundCloudProvider>>>,
    sp: Signal<Option<Arc<SpotifyProvider>>>,
}

impl UseArtist {
    /// Fetch the artist + top tracks + albums in one go. The Related tab
    /// stays unloaded until `load_related` is called — keeps initial
    /// render snappy.
    pub fn load(&self, uri: ArtistUri) {
        let sc = self.sc.peek().clone();
        let sp = self.sp.peek().clone();
        let mut view = self.view;
        let mut is_loading = self.is_loading;
        let mut error = self.error;
        spawn(async move {
            is_loading.set(true);
            error.set(None);
            view.set(None);

            let provider: Option<Arc<dyn Provider>> = match infer_provider(&uri) {
                Some(ProviderId::Spotify) => sp.map(|p| p as Arc<dyn Provider>),
                Some(ProviderId::SoundCloud) => sc.map(|p| p as Arc<dyn Provider>),
                _ => None,
            };
            let Some(provider) = provider else {
                error.set(Some("No provider available for this artist URI.".into()));
                is_loading.set(false);
                return;
            };

            // Banner — required.
            let artist = match provider.artist(&uri).await {
                Ok(a) => a,
                Err(e) => {
                    error.set(Some(format!("artist: {e}")));
                    is_loading.set(false);
                    return;
                }
            };

            // Top tracks + albums in parallel. Either can fail; we keep the
            // banner regardless and surface partial data — the artist view
            // still works with just a banner + albums if top tracks 404, etc.
            let (top, albums) = tokio::join!(
                provider.artist_top_tracks(&uri, 10),
                provider.artist_albums(&uri, 50),
            );
            let top_tracks = top.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "artist_top_tracks failed");
                Vec::new()
            });
            let albums = albums.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "artist_albums failed");
                Vec::new()
            });

            view.set(Some(ArtistView {
                artist,
                top_tracks,
                albums,
                related: None,
            }));
            is_loading.set(false);
        });
    }

    pub fn load_related(&self) {
        let Some(v) = self.view.peek().clone() else {
            return;
        };
        let uri = v.artist.uri.clone();
        let provider_id = v.artist.provider;
        let sc = self.sc.peek().clone();
        let sp = self.sp.peek().clone();
        let mut view = self.view;
        let mut is_loading_related = self.is_loading_related;
        spawn(async move {
            is_loading_related.set(true);
            let provider: Option<Arc<dyn Provider>> = match provider_id {
                ProviderId::Spotify => sp.map(|p| p as Arc<dyn Provider>),
                ProviderId::SoundCloud => sc.map(|p| p as Arc<dyn Provider>),
                ProviderId::Local => None,
            };
            let related = match provider {
                Some(p) => p.related_artists(&uri, 20).await.unwrap_or_default(),
                None => Vec::new(),
            };
            // Peek-clone-set instead of mutating in place: peek's read
            // guard would overlap view.set's mutable borrow otherwise.
            let updated = view.peek().clone().map(|mut current| {
                current.related = Some(related);
                current
            });
            if let Some(u) = updated {
                view.set(Some(u));
            }
            is_loading_related.set(false);
        });
    }
}

pub fn use_artist() -> UseArtist {
    let sc = use_context::<Arc<SoundCloudProvider>>();
    let sp = use_context::<Arc<SpotifyProvider>>();
    UseArtist {
        view: use_signal(|| None::<ArtistView>),
        is_loading: use_signal(|| false),
        is_loading_related: use_signal(|| false),
        error: use_signal(|| None::<String>),
        sc: use_signal(|| Some(sc)),
        sp: use_signal(|| Some(sp)),
    }
}

fn infer_provider(uri: &ArtistUri) -> Option<ProviderId> {
    if uri.0.starts_with("spotify:") {
        Some(ProviderId::Spotify)
    } else if uri.0.starts_with("soundcloud:") {
        Some(ProviderId::SoundCloud)
    } else {
        None
    }
}

/// Album-type split heuristic mirroring aegis: explicit `Album`/`Compilation`
/// → long-play; `Single`/`Ep` → short; `Unknown` falls back to track-count
/// (>=6 = long-play). Exported so the Discover/Library pages can use the
/// same logic if they grow album rows.
pub fn is_long_play(album: &AlbumBrief) -> bool {
    match album.album_type {
        AlbumType::Album | AlbumType::Compilation => true,
        AlbumType::Single | AlbumType::Ep => false,
        AlbumType::Unknown => album.total_tracks.unwrap_or(0) >= 6,
    }
}
