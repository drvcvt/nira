use std::sync::Arc;

use dioxus::prelude::*;
use provider_api::{PlaylistUri, Provider, ProviderError, Track};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

#[derive(Clone, Copy)]
pub struct UsePlaylist {
    pub tracks: Signal<Vec<Track>>,
    pub is_loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    generation: Signal<u64>,
    sc: Signal<Option<Arc<SoundCloudProvider>>>,
    sp: Signal<Option<Arc<SpotifyProvider>>>,
}

impl UsePlaylist {
    pub fn load(&self, uri: PlaylistUri) {
        let sc = self.sc.peek().clone();
        let sp = self.sp.peek().clone();
        let mut tracks = self.tracks;
        let mut is_loading = self.is_loading;
        let mut error = self.error;
        let mut generation = self.generation;
        let generation_at_start = generation.peek().wrapping_add(1);
        generation.set(generation_at_start);

        spawn(async move {
            is_loading.set(true);
            error.set(None);
            tracks.set(Vec::new());

            let provider: Option<Arc<dyn Provider>> = if uri.0.starts_with("soundcloud:") {
                sc.map(|p| p as Arc<dyn Provider>)
            } else if uri.0.starts_with("spotify:") {
                sp.map(|p| p as Arc<dyn Provider>)
            } else {
                None
            };
            let outcome = match provider {
                Some(provider) => provider.playlist_tracks(&uri).await,
                None => Err(ProviderError::NotAvailable),
            };
            if *generation.peek() != generation_at_start {
                return;
            }
            match outcome {
                Ok(loaded) => tracks.set(loaded),
                Err(e) => error.set(Some(format!("playlist: {e}"))),
            }
            is_loading.set(false);
        });
    }
}

pub fn use_playlist() -> UsePlaylist {
    let sc = use_context::<Arc<SoundCloudProvider>>();
    let sp = use_context::<Arc<SpotifyProvider>>();
    UsePlaylist {
        tracks: use_signal(Vec::new),
        is_loading: use_signal(|| false),
        error: use_signal(|| None::<String>),
        generation: use_signal(|| 0u64),
        sc: use_signal(|| Some(sc)),
        sp: use_signal(|| Some(sp)),
    }
}
