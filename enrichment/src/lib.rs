//! Read-only external data sources for the discovery engine.
//!
//! The discovery crate composes these clients to turn a seed track into a
//! ranked candidate list — neither client knows what discovery is doing with
//! their output, they just answer queries.
//!
//! Why two sources for what looks like one job:
//! - **MusicBrainz** has stable text search and gives us canonical recording
//!   MBIDs for any (artist, title). We use it to resolve seeds.
//! - **ListenBrainz** publishes a similarity graph derived from millions of
//!   real users' listening history. Given an MBID it returns a ranked
//!   neighbourhood including embedded artist/title metadata, so we don't
//!   have to fan out per-candidate metadata calls.
//!
//! Both are open, non-commercial, polite-UA APIs with modest rate limits.
//! We share one in-memory TTL cache across calls to keep hammering down.

pub mod cache;
pub mod lastfm;
pub mod listenbrainz;
pub mod musicbrainz;

// Re-export the inbound-listen DTO so `hooks::use_listenbrainz_feed` can
// import `enrichment::Listen` without rooting into a submodule.
pub use lastfm::LastFmSimilar;
pub use listenbrainz::Listen;

use std::sync::Arc;

use reqwest::Client;

use crate::cache::TtlCache;

#[derive(Debug, thiserror::Error)]
pub enum EnrichmentError {
    #[error("network: {0}")]
    Network(String),
    #[error("malformed response: {0}")]
    Malformed(String),
    #[error("rate limited; retry after {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("no result")]
    NotFound,
}

pub type EnrichmentResult<T> = Result<T, EnrichmentError>;

/// One client per nira instance — holds the shared HTTP pool and cache.
#[derive(Clone)]
pub struct EnrichmentClient {
    http: Client,
    cache: Arc<TtlCache>,
    lastfm_key: Option<String>,
}

impl EnrichmentClient {
    pub fn new() -> EnrichmentResult<Self> {
        Self::with_lastfm_key(None)
    }

    /// Construct with an explicit Last.fm key. Empty/whitespace input is
    /// treated as missing and falls through to the `NIRA_LASTFM_API_KEY`
    /// env var. Both missing → discovery silently skips the Last.fm path.
    pub fn with_lastfm_key(key: Option<String>) -> EnrichmentResult<Self> {
        let http = Client::builder()
            // Both MB and LB require a contact-able User-Agent. They will
            // throttle or 503 anonymous clients hard.
            .user_agent(
                "nira/0.1.0 (https://github.com/dracut/nira; cross-platform music discovery)",
            )
            .build()
            .map_err(|e| EnrichmentError::Network(e.to_string()))?;
        let lastfm_key = key
            .filter(|k| !k.trim().is_empty())
            .or_else(|| std::env::var("NIRA_LASTFM_API_KEY").ok())
            .filter(|k| !k.trim().is_empty());
        Ok(Self {
            http,
            cache: Arc::new(TtlCache::new()),
            lastfm_key,
        })
    }

    pub fn lastfm_key(&self) -> Option<&str> {
        self.lastfm_key.as_deref()
    }

    pub(crate) fn http(&self) -> &Client {
        &self.http
    }

    pub(crate) fn cache(&self) -> &TtlCache {
        &self.cache
    }
}
