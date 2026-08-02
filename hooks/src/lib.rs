//! Per-domain reactivity primitives.
//!
//! Each `use_*` returns a focused signal-set that pages can subscribe to
//! without dragging the rest of the app along for the re-render. The
//! anti-pattern this whole project is built to avoid is a single global
//! `BootstrapState` signal that every page reads — that turns every
//! domain-local mutation into an app-wide diff.

use std::sync::Arc;

use dioxus::prelude::*;
use enrichment::EnrichmentClient;
pub use provider_soundcloud::{
    SoundCloudPlaylist, SoundCloudPlaylistCatalog, SoundCloudPlaylistImport,
    SoundCloudPlaylistSummary, SoundCloudProvider, validate_soundcloud_url,
};
pub use provider_spotify::{
    SpotifyPlaylist, SpotifyPlaylistCatalog, SpotifyPlaylistImport, SpotifyPlaylistSummary,
    SpotifyProvider,
};

pub mod matching;
pub mod queue;
pub mod scrobble;
mod taste;
pub mod use_album;
pub mod use_artist;
pub mod use_ctx_menu;
pub mod use_detail;
pub mod use_discovery;
pub mod use_downloads;
pub mod use_history;
pub mod use_library;
pub mod use_likes;
pub mod use_local_library;
pub mod use_playlists;
pub mod use_playlist;
pub mod use_listenbrainz_feed;
pub mod use_player;
pub mod use_recommendations;
pub mod use_search;
pub mod use_together;
pub mod use_youtube;

pub use matching::{find_strict_match, match_key, track_match_key};
pub use queue::{RadioStatus, RepeatMode, UseQueue, use_queue};
pub use use_album::{UseAlbum, fetch_album_detail, use_album};
pub use use_artist::{ArtistView, UseArtist, is_long_play, use_artist};
pub use use_ctx_menu::{AlbumCtx, CtxMenuState, CtxTarget, UseCtxMenu, use_ctx_menu};
pub use use_detail::{DetailView, UseDetail, uri_has_detail_page, use_detail};
pub use use_discovery::{DiscoveryMode, UseDiscovery, use_discovery};
pub use use_downloads::{UseDownloads, use_downloads};
pub use use_history::{UseHistory, install_history, use_history};
pub use use_library::{UseLibrary, install_library, use_library};
pub use use_likes::{LikedTrack, UseLikes, use_likes};
pub use use_playlists::{
    Playlist, PlaylistAlbum, PlaylistImport, UsePlaylists, use_playlists,
};
pub use use_playlist::{UsePlaylist, use_playlist};
pub use use_local_library::{UseLocalLibrary, use_local_library};
pub use use_listenbrainz_feed::{UseListenBrainzFeed, use_listenbrainz_feed};
pub use use_player::{PlayerContext, UsePlayer, use_player};
pub use use_recommendations::{
    RecommendationMix, RecommendationShelf, RecommendationTile, UseRecommendations,
    use_recommendations,
};
pub use use_search::{UseSearch, use_search};
pub use use_together::{UseTogether, use_together};
pub use use_youtube::{UseYouTube, YouTubePlaylist, YouTubePreview, use_youtube};

// Re-export the player- and provider-side types pages/components consume so
// they never need to depend on those crates directly.
pub use config::{AppConfig, ThemePref, UI_FONTS, ui_font_stack};
pub use discovery::{
    CrossPlatformMatch, DiscoveryEngine, DiscoveryResult, DiscoverySourcePrefs, SimilarToSeed,
};
pub use enrichment::Listen;
pub use player::{Active, HistoryEntry, NowPlaying, Player, PlayerError, PlayerSnapshot, VizFrame};
pub use provider_api::{
    AlbumBrief, AlbumDetail, AlbumRef, AlbumType, AlbumUri, Artist, ArtistRef, ArtistUri, Provider,
    PlaylistBrief, PlaylistKind, PlaylistOpen, PlaylistUri, ProviderError, ProviderId, Query,
    RelatedArtist, Track, TrackUri,
};

/// One-shot context installer for the root `App` component. Provisions the
/// audio engine, providers, discovery engine, and the persisted-config
/// signal into Dioxus context — pages and components pull what they need
/// from there via the typed `use_*` helpers below.
pub struct AppContext;

impl AppContext {
    pub fn install(
        player: Player,
        sc: Arc<SoundCloudProvider>,
        spotify: Arc<SpotifyProvider>,
        config: AppConfig,
    ) {
        PlayerContext::install(player.clone());

        // Concrete provider handles (kept around for search/playback dispatch).
        use_context_provider({
            let sc = sc.clone();
            move || sc
        });
        use_context_provider({
            let sp = spotify.clone();
            move || sp
        });

        use_search::install_search();

        // Discovery engine — owns its own EnrichmentClient (MB + LB cache),
        // sees the providers as `Vec<Arc<dyn Provider>>` so it doesn't need
        // to know which concrete types are wired in. The Last.fm key, if any,
        // is plumbed in from config so discovery can fan a third source out;
        // unset key → Last.fm path skipped silently.
        let enrichment = Arc::new(
            EnrichmentClient::with_lastfm_key(config.lastfm_api_key.clone())
                .expect("enrichment client init"),
        );
        let providers: Vec<Arc<dyn Provider>> = vec![
            sc.clone() as Arc<dyn Provider>,
            spotify.clone() as Arc<dyn Provider>,
        ];
        let engine = Arc::new(DiscoveryEngine::new(
            enrichment.clone(),
            providers,
            sc.clone(),
            DiscoverySourcePrefs {
                soundcloud: config.discovery_soundcloud,
                listenbrainz: config.discovery_listenbrainz,
                lastfm: config.discovery_lastfm,
            },
        ));
        use_context_provider(move || engine);

        // Expose the enrichment client directly so the Home "Listened lately"
        // hook can fetch user listens without going through the discovery
        // engine (which has a different scope).
        use_context_provider({
            let e = enrichment.clone();
            move || e
        });

        // Live config signal.
        let config_sig = use_signal(|| config);
        use_context_provider(move || config_sig);

        // Queue + auto-advance watcher. Pages route track-clicks through
        // this; the watcher detects natural track-end and walks the index.
        queue::install(player.clone(), sc.clone(), spotify.clone());

        // Global context-menu signal. Track rows on any page open it; the
        // `ContextMenu` component in the app root subscribes and renders.
        use_ctx_menu::install_ctx_menu();

        // Detail-view overlay routing — when Some, the shell renders an
        // Artist or Album page instead of the active Section's content.
        use_detail::install_detail();

        // Local liked-songs store. Cross-provider (anything in a Track),
        // persisted as JSON in the config dir so cache wipes don't lose it.
        use_likes::install_likes();

        // Local playlists — same persistence tier as likes.
        use_playlists::install_playlists();

        // Provider-neutral background download status.
        use_downloads::install_downloads();

        // Play-history singleton (Recently played + recommendation seed
        // pool). Installed here so `remove` can refresh every subscriber.
        use_history::install_history(player.clone());

        // Spotify Liked Songs — singleton so the paginated sync runs once
        // at the root instead of restarting on every Home↔Library switch.
        use_library::install_library();

        // yt-dlp preview/import state. Root-owned so a download keeps running
        // when the user leaves Library.
        use_youtube::install_youtube();

        // Local-file library — scans config.library_root once on boot,
        // re-scannable from Settings/Library. Empty until a folder is set.
        use_local_library::install_local_library(config_sig);

        // Listen-together session. Publishes what we play while hosting and
        // drives the player to match the host while following. Installed last
        // because it reads the queue and library contexts the calls above
        // just provided.
        use_together::install_together(
            queue::use_queue(),
            player.clone(),
            use_local_library::use_local_library(),
        );

        // Background scrobble watcher. No-op until the user pastes a
        // ListenBrainz token in Settings.
        scrobble::install(player, enrichment, config_sig);
    }
}

pub fn use_soundcloud() -> Arc<SoundCloudProvider> {
    use_context::<Arc<SoundCloudProvider>>()
}

pub fn use_spotify() -> Arc<SpotifyProvider> {
    use_context::<Arc<SpotifyProvider>>()
}

pub fn use_enrichment() -> Arc<EnrichmentClient> {
    use_context::<Arc<EnrichmentClient>>()
}

pub fn use_discovery_engine() -> Arc<DiscoveryEngine> {
    use_context::<Arc<DiscoveryEngine>>()
}

pub fn use_config() -> Signal<AppConfig> {
    use_context::<Signal<AppConfig>>()
}

/// `m:ss`, or `h:mm:ss` once a track runs past an hour.
///
/// One shared copy: the bottombar, the fullscreen cover and every track row
/// each had their own, and all three lacked the hours branch — a 90-minute
/// SoundCloud DJ mix rendered as "90:00" and counted up through "78:42".
pub fn fmt_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod fmt_time_tests {
    use super::fmt_time;

    #[test]
    fn switches_to_hours_only_past_the_hour() {
        assert_eq!(fmt_time(0), "0:00");
        assert_eq!(fmt_time(7), "0:07");
        assert_eq!(fmt_time(605), "10:05");
        assert_eq!(fmt_time(3599), "59:59");
        // The bug: a 90-minute mix used to render as "90:00".
        assert_eq!(fmt_time(3600), "1:00:00");
        assert_eq!(fmt_time(5400), "1:30:00");
        assert_eq!(fmt_time(7322), "2:02:02");
    }
}
