//! "Same track, other platform" — given one track, find the best playable
//! match on every *other* registered provider.
//!
//! Distinct from `SimilarTo`: this mode does NOT walk a similarity
//! neighbourhood. It's a 1:1 identity bridge. Useful when a user finds a
//! track on SoundCloud and wants the Spotify version (for Connect-cast,
//! offline, or Premium-only features) or vice versa.
//!
//! Resolution is text-search on `"{artist} {title}"`. MBID-exact matching
//! would need an MB roundtrip per call — deferred to a later phase.

use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use provider_api::{Provider, ProviderId, Query, Track};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossPlatformMatch {
    /// The seed track exactly as supplied by the caller.
    pub source: Track,
    /// Best-effort match on Spotify, if Spotify is registered and finds one.
    pub spotify: Option<Track>,
    /// Best-effort match on SoundCloud, ditto.
    pub soundcloud: Option<Track>,
}

impl CrossPlatformMatch {
    /// Anything to play besides the seed itself?
    pub fn has_other_provider(&self) -> bool {
        let seed = self.source.provider;
        (seed != ProviderId::Spotify && self.spotify.is_some())
            || (seed != ProviderId::SoundCloud && self.soundcloud.is_some())
    }
}

pub(crate) async fn resolve_bridge(
    providers: &[Arc<dyn Provider>],
    source: Track,
) -> CrossPlatformMatch {
    let seed_provider = source.provider;
    let q = Query {
        text: format!(
            "{} {}",
            source
                .artists
                .iter()
                .map(|a| a.name.clone())
                .next()
                .unwrap_or_default(),
            source.title,
        ),
        limit: Some(5),
    };
    let mut spotify = None;
    let mut soundcloud = None;

    let mut futs = FuturesUnordered::new();
    for p in providers {
        // Skip the seed's own provider — we already have that track.
        if p.id() == seed_provider {
            continue;
        }
        let p = p.clone();
        let q = q.clone();
        futs.push(async move { (p.id(), p.search(&q).await) });
    }
    while let Some((id, res)) = futs.next().await {
        if let Ok(results) = res
            && let Some(top) = results.tracks.into_iter().next()
        {
            match id {
                ProviderId::Spotify => spotify = Some(top),
                ProviderId::SoundCloud => soundcloud = Some(top),
                ProviderId::Local => {}
            }
        }
    }
    CrossPlatformMatch {
        source,
        spotify,
        soundcloud,
    }
}
