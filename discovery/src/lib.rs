//! Cross-platform discovery engine.
//!
//! Takes a seed track and emits ranked candidate results, each potentially
//! playable on multiple providers. The USP of nira: a Spotify-seeded query
//! can yield SoundCloud-playable results and vice versa, with rationale
//! exposed to the UI so the user understands *why* a track was picked.
//!
//! Two candidate sources, run in parallel:
//!
//! 1. **SoundCloud's own related-tracks feed.** We resolve the seed by
//!    searching SC for "artist title", take the top hit, and pull its
//!    `/tracks/{id}/related`. SC has unmatched coverage for niche electronic
//!    so this works where LB's similarity graph is silent.
//! 2. **ListenBrainz similar-recordings.** MusicBrainz resolves the seed to
//!    an MBID; LB returns neighbouring MBIDs with co-listening scores. Great
//!    for popular catalog, sparse for underground.
//!
//! Whichever returns candidates contributes. Both contributing = the merge
//! deduplicates by (lowercase artist, lowercase title) and keeps the higher
//! score. If both empty → `EmptyNeighbourhood`.
//!
//! After candidates are merged, each is resolved on every registered
//! provider (parallel `tokio::join!`) so users see both SP and SC badges
//! per row when available.

use std::collections::HashMap;
use std::sync::Arc;

use enrichment::EnrichmentClient;
use futures::stream::{FuturesUnordered, StreamExt};
use provider_api::{Provider, ProviderError, ProviderId, Query, Track};
use provider_soundcloud::SoundCloudProvider;
use serde::{Deserialize, Serialize};

pub mod cross_platform;
pub use cross_platform::CrossPlatformMatch;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("seed track could not be resolved on any provider")]
    SeedUnresolved,
    #[error("no neighbours found for this track (yet)")]
    EmptyNeighbourhood,
    #[error("enrichment: {0}")]
    Enrichment(#[from] enrichment::EnrichmentError),
    #[error("provider: {0}")]
    Provider(#[from] ProviderError),
}

pub type DiscoveryResultSet = Result<Vec<DiscoveryResult>, DiscoveryError>;

#[derive(Debug, Clone)]
pub struct SimilarToSeed {
    pub artist: String,
    pub title: String,
    pub mbid: Option<String>,
}

impl SimilarToSeed {
    pub fn from_input(s: &str) -> Self {
        let trimmed = s.trim();
        if let Some((artist, title)) = trimmed.split_once(" - ") {
            Self {
                artist: artist.trim().to_string(),
                title: title.trim().to_string(),
                mbid: None,
            }
        } else {
            Self {
                artist: String::new(),
                title: trimmed.to_string(),
                mbid: None,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryResult {
    pub mbid: Option<String>,
    pub title: String,
    pub artist: String,
    pub cover_url: Option<String>,
    pub spotify: Option<Track>,
    pub soundcloud: Option<Track>,
    pub score: f32,
    pub rationale: String,
}

impl DiscoveryResult {
    pub fn play_target(&self) -> Option<Track> {
        self.soundcloud.clone().or_else(|| self.spotify.clone())
    }
}

/// Internal candidate before provider-resolution. Either source produces
/// these; the merge stage dedupes and the resolution stage upgrades them
/// to `DiscoveryResult`.
#[derive(Debug, Clone)]
struct Candidate {
    mbid: Option<String>,
    title: String,
    artist: String,
    /// Already in [0.0, 1.0]. SC candidates use a linear-decay synthetic
    /// score because SC's API doesn't return one; LB returns its own which
    /// we normalise into the same range.
    score: f32,
    sources: Vec<&'static str>,
}

#[derive(Clone)]
pub struct DiscoveryEngine {
    enrichment: Arc<EnrichmentClient>,
    providers: Vec<Arc<dyn Provider>>,
    /// Held directly so we can call `related_tracks`, which isn't part of
    /// the `Provider` trait. The "no concrete provider deps" architectural
    /// rule was relaxed for this — SC's related-tracks endpoint is too
    /// useful for niche discovery to skip.
    sc: Arc<SoundCloudProvider>,
}

impl DiscoveryEngine {
    pub fn new(
        enrichment: Arc<EnrichmentClient>,
        providers: Vec<Arc<dyn Provider>>,
        sc: Arc<SoundCloudProvider>,
    ) -> Self {
        Self {
            enrichment,
            providers,
            sc,
        }
    }

    /// Reports whether the Last.fm path is wired up (a key was supplied
    /// via config or env). UI uses this to render `lf off` vs `lf 0` so
    /// users can tell missing-config from empty-result.
    pub fn lastfm_configured(&self) -> bool {
        self.enrichment.lastfm_key().is_some()
    }

    /// "Tracks similar to this seed."
    pub async fn similar_to(&self, seed: SimilarToSeed) -> DiscoveryResultSet {
        let (sc_path, lb_path, lastfm_path) = tokio::join!(
            self.sc_candidates(&seed),
            self.lb_candidates(&seed),
            self.lastfm_candidates(&seed),
        );

        // Surface per-source candidate counts so launcher logs make it
        // obvious why an SC-only result list isn't a bug (LB had no MBID,
        // LF had no key, etc.) without having to bisect the engine.
        let sc_n = sc_path.as_ref().map(|v| v.len()).unwrap_or(0);
        let lb_n = lb_path.as_ref().map(|v| v.len()).unwrap_or(0);
        let lf_n = lastfm_path.as_ref().map(|v| v.len()).unwrap_or(0);
        tracing::info!(
            sc = sc_n,
            lb = lb_n,
            lf = lf_n,
            seed_artist = %seed.artist,
            seed_title = %seed.title,
            "discovery candidates by source"
        );

        let mut buckets: HashMap<String, Candidate> = HashMap::new();
        for source_set in [sc_path, lb_path, lastfm_path].into_iter().flatten() {
            for c in source_set {
                let key = dedupe_key(&c.artist, &c.title);
                buckets
                    .entry(key)
                    .and_modify(|existing| {
                        if c.score > existing.score {
                            existing.score = c.score;
                        }
                        if existing.mbid.is_none() {
                            existing.mbid = c.mbid.clone();
                        }
                        for s in &c.sources {
                            if !existing.sources.contains(s) {
                                existing.sources.push(s);
                            }
                        }
                    })
                    .or_insert(c);
            }
        }

        let candidates: Vec<Candidate> = buckets.into_values().collect();
        if candidates.is_empty() {
            return Err(DiscoveryError::EmptyNeighbourhood);
        }

        // Resolve playability across all providers in parallel.
        let providers = self.providers.clone();
        let mut futs = FuturesUnordered::new();
        for c in candidates.into_iter() {
            let providers = providers.clone();
            futs.push(async move {
                let q = Query {
                    text: format!("{} {}", c.artist, c.title),
                    limit: Some(5),
                };
                let resolved = resolve_on_providers(&providers, &q).await;
                if resolved.spotify.is_none() && resolved.soundcloud.is_none() {
                    None
                } else {
                    Some(DiscoveryResult {
                        mbid: c.mbid.clone(),
                        cover_url: resolved
                            .soundcloud
                            .as_ref()
                            .and_then(|t| t.cover_url.clone())
                            .or_else(|| {
                                resolved.spotify.as_ref().and_then(|t| t.cover_url.clone())
                            }),
                        title: c.title.clone(),
                        artist: c.artist.clone(),
                        spotify: resolved.spotify,
                        soundcloud: resolved.soundcloud,
                        score: c.score,
                        rationale: format!(
                            "{} · score {:.2}",
                            c.sources.join(" + "),
                            c.score
                        ),
                    })
                }
            });
        }

        let mut out: Vec<DiscoveryResult> = Vec::new();
        while let Some(item) = futs.next().await {
            if let Some(r) = item {
                out.push(r);
            }
        }
        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(30);
        if out.is_empty() {
            return Err(DiscoveryError::EmptyNeighbourhood);
        }
        Ok(out)
    }

    /// Find the same track on every *other* provider. Returns the seed
    /// untouched plus best-effort matches; the UI decides what to render
    /// based on `CrossPlatformMatch::has_other_provider`.
    pub async fn cross_platform_bridge(&self, source: Track) -> CrossPlatformMatch {
        cross_platform::resolve_bridge(&self.providers, source).await
    }

    /// Resolve seed → SC track → SC related. Empty Vec if SC can't find the
    /// seed at all; an Err only if the SC API itself misbehaves.
    async fn sc_candidates(
        &self,
        seed: &SimilarToSeed,
    ) -> Result<Vec<Candidate>, DiscoveryError> {
        let query = if seed.artist.is_empty() {
            seed.title.clone()
        } else {
            format!("{} {}", seed.artist, seed.title)
        };
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let search = self
            .sc
            .search(&Query {
                text: query,
                limit: Some(5),
            })
            .await?;
        let Some(top) = search.tracks.first().cloned() else {
            return Ok(Vec::new());
        };
        let related = self.sc.related_tracks(&top.uri, 30).await?;
        let len = related.len().max(1) as f32;
        Ok(related
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                // Linear decay from 1.0 to 0.5 — top of SC's list is most
                // confident, but we never let SC candidates fall below the
                // mid-tier so they always beat unscored LB stragglers.
                let score = 1.0 - 0.5 * (i as f32 / len);
                Candidate {
                    mbid: None,
                    artist: t
                        .artists
                        .iter()
                        .map(|a| a.name.clone())
                        .next()
                        .unwrap_or_default(),
                    title: t.title,
                    score,
                    sources: vec!["SoundCloud"],
                }
            })
            .collect())
    }

    /// MB resolve → LB similar. Returns empty when LB has no neighbourhood
    /// data for the seed's MBID (common for niche electronic).
    async fn lb_candidates(
        &self,
        seed: &SimilarToSeed,
    ) -> Result<Vec<Candidate>, DiscoveryError> {
        let mbid = match seed.mbid.clone() {
            Some(m) => m,
            None => match self.resolve_seed_mbid(seed).await {
                Ok(m) => m,
                Err(_) => return Ok(Vec::new()),
            },
        };
        let neighbours = match self.enrichment.lb_similar_recordings(&mbid, 30).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "LB similar-recordings failed; skipping path");
                return Ok(Vec::new());
            }
        };
        if neighbours.is_empty() {
            return Ok(Vec::new());
        }
        let max_score = neighbours
            .iter()
            .map(|n| n.score)
            .fold(0.0f32, f32::max)
            .max(1.0);
        Ok(neighbours
            .into_iter()
            .map(|n| Candidate {
                mbid: Some(n.mbid),
                title: n.title,
                artist: n.artist,
                score: (n.score / max_score).clamp(0.0, 1.0),
                sources: vec!["ListenBrainz"],
            })
            .collect())
    }

    /// Last.fm `track.getSimilar` neighbourhood. Empty when no key is
    /// configured or Last.fm has nothing for the seed; Err only on real
    /// network/malformed errors.
    async fn lastfm_candidates(
        &self,
        seed: &SimilarToSeed,
    ) -> Result<Vec<Candidate>, DiscoveryError> {
        if seed.artist.is_empty() || seed.title.is_empty() {
            return Ok(Vec::new());
        }
        let key = self.enrichment.lastfm_key();
        if key.is_none() {
            return Ok(Vec::new());
        }
        let similar = match self
            .enrichment
            .lastfm_similar_tracks(key.as_deref(), &seed.artist, &seed.title, 30)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "Last.fm similar failed; skipping path");
                return Ok(Vec::new());
            }
        };
        Ok(similar
            .into_iter()
            .map(|s| Candidate {
                mbid: None,
                title: s.title,
                artist: s.artist,
                score: s.score.clamp(0.0, 1.0),
                sources: vec!["Last.fm"],
            })
            .collect())
    }

    async fn resolve_seed_mbid(&self, seed: &SimilarToSeed) -> Result<String, DiscoveryError> {
        let hits = self
            .enrichment
            .mb_search_recording(&seed.artist, &seed.title, 3)
            .await?;
        if let Some(top) = hits.into_iter().next() {
            return Ok(top.mbid);
        }
        if seed.artist.is_empty() && !seed.title.is_empty() {
            let hits = self
                .enrichment
                .mb_search_recording(&seed.title, "", 3)
                .await?;
            if let Some(top) = hits.into_iter().next() {
                return Ok(top.mbid);
            }
        }
        Err(DiscoveryError::SeedUnresolved)
    }
}

fn dedupe_key(artist: &str, title: &str) -> String {
    format!("{}|{}", artist.to_lowercase(), title.to_lowercase())
}

struct ResolvedAcrossProviders {
    spotify: Option<Track>,
    soundcloud: Option<Track>,
}

async fn resolve_on_providers(
    providers: &[Arc<dyn Provider>],
    q: &Query,
) -> ResolvedAcrossProviders {
    let mut spotify = None;
    let mut soundcloud = None;
    let mut futs = FuturesUnordered::new();
    for p in providers {
        let p = p.clone();
        let q = q.clone();
        futs.push(async move {
            let id = p.id();
            let res = p.search(&q).await;
            (id, res)
        });
    }
    while let Some((id, res)) = futs.next().await {
        if let Ok(results) = res
            && let Some(top) = pick_best_match(&results.tracks, q)
        {
            match id {
                ProviderId::Spotify => spotify = Some(top),
                ProviderId::SoundCloud => soundcloud = Some(top),
                ProviderId::Local => {}
            }
        }
    }
    ResolvedAcrossProviders {
        spotify,
        soundcloud,
    }
}

fn pick_best_match(tracks: &[Track], _q: &Query) -> Option<Track> {
    tracks.first().cloned()
}

pub use provider_api::TrackUri as DiscoveryTrackUri;
