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
    AlbumBrief, AlbumType, Artist, ArtistUri, Provider, ProviderId, Query, RelatedArtist, Track,
};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

use crate::matching::match_key;

/// Banner + Top Tracks load together; the rest of the view fetches lazily.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtistView {
    pub artist: Artist,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<AlbumBrief>,
    /// Spotify artist URI this view was augmented from. Set when the native
    /// provider had no album catalogue (SoundCloud) and a strict name match
    /// on Spotify filled the gap — Related rides this alias too.
    pub via_spotify: Option<ArtistUri>,
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
    /// Bumped per `load` so a slow response for artist A can never overwrite
    /// a faster navigation to artist B — last navigation wins, always.
    generation: Signal<u64>,
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
        let mut is_loading_related = self.is_loading_related;
        let mut error = self.error;
        let mut generation = self.generation;
        let generation_at_start = generation.peek().wrapping_add(1);
        generation.set(generation_at_start);
        spawn(async move {
            is_loading.set(true);
            // Any in-flight related fetch belongs to the previous artist now.
            is_loading_related.set(false);
            error.set(None);
            view.set(None);

            let sp_aug = sp.clone();
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

            // Banner + top tracks + albums in ONE join — the view is set
            // once at the end anyway, so serialising the banner fetch ahead
            // of the other two only added a full round-trip of latency.
            // Only the banner is required; the other two can fail and the
            // page still works with partial data.
            let t0 = std::time::Instant::now();
            let (artist_res, top, albums) = tokio::join!(
                provider.artist(&uri),
                provider.artist_top_tracks(&uri, 10),
                provider.artist_albums(&uri, 50),
            );
            let mut artist = match artist_res {
                Ok(a) => a,
                Err(e) => {
                    if *generation.peek() == generation_at_start {
                        error.set(Some(format!("artist: {e}")));
                        is_loading.set(false);
                    }
                    return;
                }
            };
            let mut top_tracks = top.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "artist_top_tracks failed");
                Vec::new()
            });
            let mut albums = albums.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "artist_albums failed");
                Vec::new()
            });

            // Cross-provider fill: a SoundCloud profile has no album
            // catalogue, so on its own the page renders bare even when the
            // discography exists one provider over. Strict name match on
            // Spotify, then borrow its albums (and top tracks/genres if the
            // native ones came back empty).
            let mut via_spotify = None;
            if albums.is_empty() && artist.provider != ProviderId::Spotify {
                if let Some(sp) = sp_aug {
                    let t_twin = std::time::Instant::now();
                    if let Some(twin) = find_spotify_twin(&sp, &artist.name).await {
                        let (sp_top, sp_albums) = tokio::join!(
                            sp.artist_top_tracks(&twin.uri, 10),
                            sp.artist_albums(&twin.uri, 50),
                        );
                        albums = sp_albums.unwrap_or_default();
                        if top_tracks.is_empty() {
                            top_tracks = sp_top.unwrap_or_default();
                        }
                        if artist.genres.is_empty() {
                            artist.genres = twin.genres;
                        }
                        via_spotify = Some(twin.uri);
                    }
                    tracing::info!(
                        ms = t_twin.elapsed().as_millis() as u64,
                        "artist spotify twin fill"
                    );
                }
            }

            if *generation.peek() != generation_at_start {
                return; // superseded by a newer navigation
            }
            tracing::info!(
                ms = t0.elapsed().as_millis() as u64,
                provider = ?artist.provider,
                "artist page loaded"
            );
            view.set(Some(ArtistView {
                artist,
                top_tracks,
                albums,
                via_spotify,
                related: None,
            }));
            is_loading.set(false);
        });
    }

    pub fn load_related(&self) {
        let Some(v) = self.view.peek().clone() else {
            return;
        };
        let provider_id = v.artist.provider;
        // Only Spotify has a related-artists feed — non-Spotify views ride
        // the augmentation alias when one was found, otherwise stay empty.
        let uri = match provider_id {
            ProviderId::Spotify => Some(v.artist.uri.clone()),
            ProviderId::SoundCloud | ProviderId::Local => {
                v.via_spotify.clone()
            }
        };
        let sp = self.sp.peek().clone();
        let mut view = self.view;
        let mut is_loading_related = self.is_loading_related;
        let generation = self.generation;
        let generation_at_start = *generation.peek();
        spawn(async move {
            is_loading_related.set(true);
            let related = match (sp, uri) {
                (Some(p), Some(uri)) => p.related_artists(&uri, 20).await.unwrap_or_default(),
                _ => Vec::new(),
            };
            if *generation.peek() != generation_at_start {
                // A newer artist navigation superseded this fetch — don't
                // attach the old artist's related list to the new view (and
                // leave the spinner alone: `load` already reset it).
                return;
            }
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
        generation: use_signal(|| 0u64),
        sc: use_signal(|| Some(sc)),
        sp: use_signal(|| Some(sp)),
    }
}

/// Strict-name Spotify twin lookup for cross-provider augmentation. A wrong
/// match puts the wrong discography on the page, so only an exact
/// normalized-name hit counts — no fuzzy fallback.
async fn find_spotify_twin(sp: &Arc<SpotifyProvider>, name: &str) -> Option<Artist> {
    if !sp.is_connected() {
        return None;
    }
    let key = match_key(name);
    if key.is_empty() {
        return None;
    }
    let q = Query {
        text: name.to_string(),
        limit: Some(6),
    };
    let results = match sp.search(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "spotify twin search failed");
            return None;
        }
    };
    results
        .artists
        .into_iter()
        .find(|a| match_key(&a.name) == key)
}

fn infer_provider(uri: &ArtistUri) -> Option<ProviderId> {
    if uri.0.starts_with("spotify:") {
        Some(ProviderId::Spotify)
    } else if uri.0.starts_with("soundcloud:") {
        Some(ProviderId::SoundCloud)
    } else {
        // local: URIs are name-derived keys, not provider entities — the UI
        // renders them as plain text (see `uri_has_detail_page`).
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
