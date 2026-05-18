//! Album detail — cover + tracklist. Single-provider: the URI prefix picks
//! which provider serves the request.

use std::sync::Arc;

use dioxus::prelude::*;
use provider_api::{AlbumDetail, AlbumUri, Provider};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

#[derive(Clone, Copy)]
pub struct UseAlbum {
    pub view: Signal<Option<AlbumDetail>>,
    pub is_loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    sc: Signal<Option<Arc<SoundCloudProvider>>>,
    sp: Signal<Option<Arc<SpotifyProvider>>>,
}

impl UseAlbum {
    pub fn load(&self, uri: AlbumUri) {
        let sc = self.sc.peek().clone();
        let sp = self.sp.peek().clone();
        let mut view = self.view;
        let mut is_loading = self.is_loading;
        let mut error = self.error;
        spawn(async move {
            is_loading.set(true);
            error.set(None);
            view.set(None);
            let provider: Option<Arc<dyn Provider>> = if uri.0.starts_with("spotify:") {
                sp.map(|p| p as Arc<dyn Provider>)
            } else if uri.0.starts_with("soundcloud:") {
                sc.map(|p| p as Arc<dyn Provider>)
            } else {
                None
            };
            let Some(provider) = provider else {
                error.set(Some("No provider for this album URI.".into()));
                is_loading.set(false);
                return;
            };
            match provider.album(&uri).await {
                Ok(d) => view.set(Some(d)),
                Err(e) => {
                    error.set(Some(format!("album: {e}")));
                }
            }
            is_loading.set(false);
        });
    }
}

pub fn use_album() -> UseAlbum {
    let sc = use_context::<Arc<SoundCloudProvider>>();
    let sp = use_context::<Arc<SpotifyProvider>>();
    UseAlbum {
        view: use_signal(|| None::<AlbumDetail>),
        is_loading: use_signal(|| false),
        error: use_signal(|| None::<String>),
        sc: use_signal(|| Some(sc)),
        sp: use_signal(|| Some(sp)),
    }
}

// Re-export ProviderId so callers don't need to root into provider_api just
// to render the provider label in an album header.
pub use provider_api::ProviderId as AlbumProviderId;
