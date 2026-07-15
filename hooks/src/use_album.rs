//! Album detail — cover + tracklist. Single-provider: the URI prefix picks
//! which provider serves the request; `local:album:` resolves synchronously
//! from the scanned library.

use std::sync::Arc;

use dioxus::prelude::*;
use provider_api::{
    AlbumDetail, AlbumType, AlbumUri, ArtistRef, ArtistUri, Provider, ProviderId, Track,
};
use provider_hires-provider::the hi-res providerProvider;
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

use crate::use_local_library::{UseLocalLibrary, use_local_library};

#[derive(Clone, Copy)]
pub struct UseAlbum {
    pub view: Signal<Option<AlbumDetail>>,
    pub is_loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// Bumped per `load` so a slow response for album A can never overwrite
    /// a faster navigation to album B — last navigation wins, always.
    generation: Signal<u64>,
    sc: Signal<Option<Arc<SoundCloudProvider>>>,
    sp: Signal<Option<Arc<SpotifyProvider>>>,
    qz: Signal<Option<Arc<the hi-res providerProvider>>>,
    local: UseLocalLibrary,
}

impl UseAlbum {
    pub fn load(&self, uri: AlbumUri) {
        let sc = self.sc.peek().clone();
        let sp = self.sp.peek().clone();
        let qz = self.qz.peek().clone();
        let local = self.local;
        let mut view = self.view;
        let mut is_loading = self.is_loading;
        let mut error = self.error;
        let mut generation = self.generation;
        let generation_at_start = generation.peek().wrapping_add(1);
        generation.set(generation_at_start);
        spawn(async move {
            is_loading.set(true);
            error.set(None);
            view.set(None);

            // Local albums resolve from the scanned library — no provider,
            // no network, no staleness window.
            if uri.0.starts_with("local:album:") {
                match local_album_detail(&local, &uri) {
                    Some(detail) => view.set(Some(detail)),
                    None => error.set(Some(
                        "Album not found in the local library — try a rescan.".into(),
                    )),
                }
                is_loading.set(false);
                return;
            }

            let provider: Option<Arc<dyn Provider>> = if uri.0.starts_with("spotify:") {
                sp.map(|p| p as Arc<dyn Provider>)
            } else if uri.0.starts_with("soundcloud:") {
                sc.map(|p| p as Arc<dyn Provider>)
            } else if uri.0.starts_with("hires-provider:") {
                qz.map(|p| p as Arc<dyn Provider>)
            } else {
                None
            };
            let Some(provider) = provider else {
                error.set(Some("No provider for this album URI.".into()));
                is_loading.set(false);
                return;
            };
            let outcome = provider.album(&uri).await;
            if *generation.peek() != generation_at_start {
                return; // superseded by a newer navigation
            }
            match outcome {
                Ok(d) => view.set(Some(d)),
                Err(e) => {
                    error.set(Some(format!("album: {e}")));
                }
            }
            is_loading.set(false);
        });
    }
}

/// Build an [`AlbumDetail`] for a `local:album:` URI out of the scanned
/// library. Tracks arrive pre-sorted disc → track from the scanner.
fn local_album_detail(local: &UseLocalLibrary, uri: &AlbumUri) -> Option<AlbumDetail> {
    let tracks: Vec<Track> = local
        .tracks
        .peek()
        .iter()
        .filter(|t| t.album.as_ref().is_some_and(|a| &a.uri == uri))
        .cloned()
        .collect();
    let first = tracks.first()?;
    Some(AlbumDetail {
        uri: uri.clone(),
        provider: ProviderId::Local,
        title: first
            .album
            .as_ref()
            .map(|a| a.title.clone())
            .unwrap_or_default(),
        artist: first.artists.first().cloned().unwrap_or(ArtistRef {
            uri: ArtistUri(String::new()),
            name: String::new(),
        }),
        cover_url: first.cover_url.clone(),
        release_year: None,
        album_type: AlbumType::Unknown,
        tracks,
    })
}

pub fn use_album() -> UseAlbum {
    let sc = use_context::<Arc<SoundCloudProvider>>();
    let sp = use_context::<Arc<SpotifyProvider>>();
    let qz = use_context::<Arc<the hi-res providerProvider>>();
    let local = use_local_library();
    UseAlbum {
        view: use_signal(|| None::<AlbumDetail>),
        is_loading: use_signal(|| false),
        error: use_signal(|| None::<String>),
        generation: use_signal(|| 0u64),
        sc: use_signal(|| Some(sc)),
        sp: use_signal(|| Some(sp)),
        qz: use_signal(|| Some(qz)),
        local,
    }
}

// Re-export ProviderId so callers don't need to root into provider_api just
// to render the provider label in an album header.
pub use provider_api::ProviderId as AlbumProviderId;
