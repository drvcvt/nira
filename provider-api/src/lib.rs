//! Common surface every streaming/local provider speaks.
//!
//! The discovery engine and the UI both program against this trait — never
//! against `provider-spotify` or `provider-soundcloud` directly. Asymmetric
//! features (SoundCloud reposts, Spotify playlists) live outside the trait
//! and are discovered via [`ProviderCaps`] so the UI can render conditional
//! surfaces without `match` ladders on a concrete provider id.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("authentication required or expired")]
    AuthRequired,
    #[error("track not available on this provider")]
    NotAvailable,
    #[error("network error: {0}")]
    Network(String),
    #[error("rate limited; retry after {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("provider returned malformed response: {0}")]
    Malformed(String),
    #[error("{0}")]
    Other(String),
}

pub type ProviderResult<T> = Result<T, ProviderError>;

/// Stable identifier for a provider implementation. New providers extend this
/// enum so the discovery engine can deduplicate results across sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    Spotify,
    SoundCloud,
    Local,
    /// Keeps persisted tracks readable after their provider is removed.
    #[serde(other)]
    Unavailable,
}

impl ProviderId {
    pub fn label(self) -> &'static str {
        match self {
            ProviderId::Spotify => "Spotify",
            ProviderId::SoundCloud => "SoundCloud",
            ProviderId::Local => "Local",
            ProviderId::Unavailable => "Unavailable",
        }
    }

    /// Short two-letter glyph for the provider badge in the UI.
    pub fn badge(self) -> &'static str {
        match self {
            ProviderId::Spotify => "S",
            ProviderId::SoundCloud => "SC",
            ProviderId::Local => "L",
            ProviderId::Unavailable => "?",
        }
    }
}

/// Provider-asymmetric feature flags. The UI inspects these to decide which
/// surfaces (e.g. a Reposts tab) to render at all.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderCaps {
    /// Has a follower-graph "stream" feed (SoundCloud).
    pub stream_feed: bool,
    /// Exposes audio-features (energy/tempo/key). Spotify deprecated this for
    /// new apps in Nov 2024 — keep the bit so we can opt in if it ever returns.
    pub audio_features: bool,
    /// Exposes user-curated playlists.
    pub playlists: bool,
    /// Supports reposts (SoundCloud).
    pub reposts: bool,
    /// Can act as a sink for cross-platform "play this elsewhere" actions.
    pub playable: bool,
}

/// Opaque provider-scoped URI. Format is `provider:type:id` — e.g.
/// `spotify:track:6rqhFgbbKwnb9MLmUQDhG6`. The bridge layer treats these as
/// black boxes; only the owning provider deserialises them.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackUri(pub String);

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistUri(pub String);

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumUri(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub uri: TrackUri,
    pub provider: ProviderId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration: Duration,
    pub cover_url: Option<String>,
    /// MusicBrainz Recording ID, when known. Cross-platform bridging keys on
    /// this when present; falls back to (artist, title) fuzzy match otherwise.
    pub mbid: Option<String>,
    /// When the track entered the user's collection on the owning provider.
    /// Spotify populates this from `/me/tracks.items[].added_at`; other
    /// providers leave it `None`. Home's "Recently liked" sorts on this.
    #[serde(default)]
    pub added_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub uri: ArtistUri,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    pub uri: AlbumUri,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub uri: ArtistUri,
    pub provider: ProviderId,
    pub name: String,
    pub image_url: Option<String>,
    /// Optional bag of provider-supplied genre strings. Spotify exposes
    /// these; SC does not. Used in the artist banner to show genre chips.
    #[serde(default)]
    pub genres: Vec<String>,
    /// Web URL where the user can open this artist in the source app.
    #[serde(default)]
    pub permalink_url: Option<String>,
}

/// Album type classification — used by Artist view to split full releases
/// from singles + EPs without an extra round trip. Providers map their
/// own taxonomy onto this; `Unknown` means "fall back to track-count
/// heuristic in the UI."
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlbumType {
    Album,
    Single,
    Ep,
    Compilation,
    #[default]
    Unknown,
}

/// Lightweight album reference — what we get back when listing an artist's
/// catalogue. Enough to render a grid card; the full tracklist lives behind
/// a separate `album(uri)` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumBrief {
    pub uri: AlbumUri,
    pub provider: ProviderId,
    pub title: String,
    pub artist_name: String,
    pub cover_url: Option<String>,
    pub release_year: Option<u32>,
    pub total_tracks: Option<u32>,
    #[serde(default)]
    pub album_type: AlbumType,
}

/// Full album view — cover + tracklist. Returned from `Provider::album(uri)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumDetail {
    pub uri: AlbumUri,
    pub provider: ProviderId,
    pub title: String,
    pub artist: ArtistRef,
    pub cover_url: Option<String>,
    pub release_year: Option<u32>,
    #[serde(default)]
    pub album_type: AlbumType,
    pub tracks: Vec<Track>,
}

/// "Find more like this artist" result row. The discovery side of the
/// artist page uses these; UI shows provider badge + name + tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedArtist {
    pub uri: ArtistUri,
    pub provider: ProviderId,
    pub name: String,
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub text: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
}

/// Opaque audio source the player crate knows how to consume. Concrete shapes
/// (librespot session, SoundCloud HTTP stream, decoded local file) land when
/// the first real provider implementation does — this enum is the bridge.
#[derive(Debug)]
pub enum StreamHandle {
    /// HTTP stream URL + content-type hint. Used by SoundCloud progressive
    /// transcodings — the caller fetches the URL into bytes itself.
    HttpStream {
        url: String,
        content_type: Option<String>,
    },
    /// Fully-resolved audio buffer. Used when the provider has to materialise
    /// the bytes itself because no single URL is sufficient — currently
    /// SoundCloud HLS (m3u8 + N segments concatenated). The caller hands the
    /// buffer straight to the audio engine without an extra HTTP roundtrip.
    Bytes {
        data: Vec<u8>,
        content_type: Option<String>,
    },
    /// In-process session, e.g. librespot. The provider holds the live session
    /// and gives the player a handle to attach to the mixer.
    InProcess { session_id: u64 },
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn caps(&self) -> ProviderCaps;
    async fn search(&self, q: &Query) -> ProviderResult<SearchResults>;
    async fn track(&self, uri: &TrackUri) -> ProviderResult<Track>;
    async fn artist(&self, uri: &ArtistUri) -> ProviderResult<Artist>;
    async fn resolve_stream(&self, uri: &TrackUri) -> ProviderResult<StreamHandle>;

    /// Artist "top tracks" — the popular-on-this-platform list. Spotify has
    /// a dedicated endpoint; SC fakes this with the user's own tracks
    /// endpoint. Default impl returns NotAvailable so providers opt in.
    async fn artist_top_tracks(&self, _uri: &ArtistUri, _limit: u32) -> ProviderResult<Vec<Track>> {
        Err(ProviderError::NotAvailable)
    }

    /// Artist's full catalogue of albums/singles/EPs. UI splits by
    /// `AlbumType`. Default opt-out for providers that don't have a real
    /// album concept (SC).
    async fn artist_albums(
        &self,
        _uri: &ArtistUri,
        _limit: u32,
    ) -> ProviderResult<Vec<AlbumBrief>> {
        Err(ProviderError::NotAvailable)
    }

    /// Full album: cover + complete tracklist. Default not-available so
    /// providers without an album concept stay silent.
    async fn album(&self, _uri: &AlbumUri) -> ProviderResult<AlbumDetail> {
        Err(ProviderError::NotAvailable)
    }

    /// Provider-native "related artists" feed, if any. Discovery's
    /// cross-provider merge layer is the right place to combine these
    /// across providers — this method only returns the calling provider's
    /// view. Default not-available.
    async fn related_artists(
        &self,
        _uri: &ArtistUri,
        _limit: u32,
    ) -> ProviderResult<Vec<RelatedArtist>> {
        Err(ProviderError::NotAvailable)
    }
}
