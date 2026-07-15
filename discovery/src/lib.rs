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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use enrichment::EnrichmentClient;
use futures::stream::{FuturesUnordered, StreamExt};
use provider_api::{Provider, ProviderError, ProviderId, Query, Track};
use provider_soundcloud::SoundCloudProvider;
use serde::{Deserialize, Serialize};

const SC_CORE_MIN: usize = 12;
const MAX_DISCOVERY_RESULTS: usize = 30;

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
    /// Already in [0.0, 1.0], after source-specific weighting.
    score: f32,
    sources: Vec<&'static str>,
    /// Exact SoundCloud related-track hit. Keeping this avoids text-searching
    /// back into random remixes/reuploads after SC already gave us the right row.
    soundcloud_track: Option<Track>,
    source_rank: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoverySourcePrefs {
    pub soundcloud: bool,
    pub listenbrainz: bool,
    pub lastfm: bool,
}

impl Default for DiscoverySourcePrefs {
    fn default() -> Self {
        Self {
            soundcloud: true,
            listenbrainz: false,
            lastfm: true,
        }
    }
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
    source_prefs: Arc<RwLock<DiscoverySourcePrefs>>,
}

impl DiscoveryEngine {
    pub fn new(
        enrichment: Arc<EnrichmentClient>,
        providers: Vec<Arc<dyn Provider>>,
        sc: Arc<SoundCloudProvider>,
        source_prefs: DiscoverySourcePrefs,
    ) -> Self {
        Self {
            enrichment,
            providers,
            sc,
            source_prefs: Arc::new(RwLock::new(source_prefs)),
        }
    }

    pub fn source_prefs(&self) -> DiscoverySourcePrefs {
        self.source_prefs.read().map(|p| *p).unwrap_or_default()
    }

    pub fn set_source_prefs(&self, prefs: DiscoverySourcePrefs) {
        if let Ok(mut guard) = self.source_prefs.write() {
            *guard = prefs;
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
        let prefs = self.source_prefs();
        let (sc_path, lb_path, lastfm_path) = tokio::join!(
            async {
                if prefs.soundcloud {
                    self.sc_candidates(&seed).await
                } else {
                    Ok(Vec::new())
                }
            },
            async {
                if prefs.listenbrainz {
                    self.lb_candidates(&seed).await
                } else {
                    Ok(Vec::new())
                }
            },
            async {
                if prefs.lastfm {
                    self.lastfm_candidates(&seed).await
                } else {
                    Ok(Vec::new())
                }
            },
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
            soundcloud_enabled = prefs.soundcloud,
            listenbrainz_enabled = prefs.listenbrainz,
            lastfm_enabled = prefs.lastfm,
            "discovery candidates by source"
        );

        let mut sc_core = clean_candidates(sc_path.unwrap_or_default(), &seed);
        let mut support = support_candidates(
            lb_path.unwrap_or_default(),
            lastfm_path.unwrap_or_default(),
            &seed,
        );
        for c in &mut sc_core {
            if let Some(s) = support.remove(&dedupe_key(&c.artist, &c.title)) {
                merge_support_into_sc(c, s);
            }
        }
        sort_candidates(&mut sc_core);

        let mut candidates = sc_core;
        if candidates.len() < SC_CORE_MIN {
            let mut fallback: Vec<Candidate> = support.into_values().collect();
            sort_candidates(&mut fallback);
            for c in fallback {
                if candidates.len() >= MAX_DISCOVERY_RESULTS {
                    break;
                }
                if !candidates.iter().any(|existing| {
                    dedupe_key(&existing.artist, &existing.title) == dedupe_key(&c.artist, &c.title)
                }) {
                    candidates.push(c);
                }
            }
        }

        candidates.truncate(MAX_DISCOVERY_RESULTS);
        if candidates.is_empty() {
            return Err(DiscoveryError::EmptyNeighbourhood);
        }

        // Resolve playability across all providers in parallel.
        let providers = self.providers.clone();
        let mut futs = FuturesUnordered::new();
        for c in candidates.into_iter() {
            let providers = providers.clone();
            futs.push(async move {
                let resolved = resolve_candidate(&providers, &c).await;
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
                        rationale: format!("{} · score {:.2}", c.sources.join(" + "), c.score),
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
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut seen_titles = HashSet::<String>::new();
        out.retain(|r| seen_titles.insert(canonical_title(&r.title)));
        out.truncate(MAX_DISCOVERY_RESULTS);
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
    async fn sc_candidates(&self, seed: &SimilarToSeed) -> Result<Vec<Candidate>, DiscoveryError> {
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
        let related = self.sc.related_tracks(&top.uri, 50).await?;
        let len = related.len().max(1) as f32;
        Ok(related
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                // SoundCloud's native order is the Aegis magic. Keep it
                // dominant and only decay to mid-tier.
                let score = 1.0 - 0.38 * (i as f32 / len);
                let artist = t
                    .artists
                    .iter()
                    .map(|a| a.name.clone())
                    .next()
                    .unwrap_or_default();
                Candidate {
                    mbid: None,
                    title: t.title.clone(),
                    artist,
                    score,
                    sources: vec!["SoundCloud"],
                    soundcloud_track: Some(t),
                    source_rank: i,
                }
            })
            .collect())
    }

    /// MB resolve → LB similar. Returns empty when LB has no neighbourhood
    /// data for the seed's MBID (common for niche electronic).
    async fn lb_candidates(&self, seed: &SimilarToSeed) -> Result<Vec<Candidate>, DiscoveryError> {
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
            .enumerate()
            .map(|(i, n)| Candidate {
                mbid: Some(n.mbid),
                title: n.title,
                artist: n.artist,
                score: ((n.score / max_score).clamp(0.0, 1.0) * 0.50).min(0.50),
                sources: vec!["ListenBrainz"],
                soundcloud_track: None,
                source_rank: i,
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
            .enumerate()
            .map(|(i, s)| Candidate {
                mbid: None,
                title: s.title,
                artist: s.artist,
                score: (s.score.clamp(0.0, 1.0) * 0.60).min(0.60),
                sources: vec!["Last.fm"],
                soundcloud_track: None,
                source_rank: i,
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

fn clean_candidates(candidates: Vec<Candidate>, seed: &SimilarToSeed) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut seen = HashSet::<String>::new();
    for c in candidates {
        if is_seed_reupload(&c, seed) || is_low_quality_variant(&c.title, seed) {
            continue;
        }
        if seen.insert(dedupe_key(&c.artist, &c.title)) {
            out.push(c);
        }
    }
    out
}

fn support_candidates(
    lb_candidates: Vec<Candidate>,
    lastfm_candidates: Vec<Candidate>,
    seed: &SimilarToSeed,
) -> HashMap<String, Candidate> {
    let mut buckets = HashMap::new();
    for c in clean_candidates(lb_candidates, seed)
        .into_iter()
        .chain(clean_candidates(lastfm_candidates, seed))
    {
        let key = dedupe_key(&c.artist, &c.title);
        buckets
            .entry(key)
            .and_modify(|existing| merge_candidate(existing, c.clone()))
            .or_insert(c);
    }
    buckets
}

fn sort_candidates(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.source_rank.cmp(&b.source_rank))
    });
}

fn merge_candidate(existing: &mut Candidate, incoming: Candidate) {
    if existing.soundcloud_track.is_some() {
        merge_support_into_sc(existing, incoming);
        return;
    }
    if incoming.score > existing.score {
        existing.score = incoming.score;
        existing.title = incoming.title.clone();
        existing.artist = incoming.artist.clone();
        existing.source_rank = incoming.source_rank;
        existing.soundcloud_track = incoming.soundcloud_track.clone();
    }
    existing.score = (existing.score + 0.05).min(0.68);
    if existing.mbid.is_none() {
        existing.mbid = incoming.mbid.clone();
    }
    for s in &incoming.sources {
        if !existing.sources.contains(s) {
            existing.sources.push(s);
        }
    }
}

fn merge_support_into_sc(existing: &mut Candidate, incoming: Candidate) {
    existing.score = (existing.score + 0.05).min(1.0);
    if existing.mbid.is_none() {
        existing.mbid = incoming.mbid.clone();
    }
    for s in &incoming.sources {
        if !existing.sources.contains(s) {
            existing.sources.push(s);
        }
    }
}

fn dedupe_key(artist: &str, title: &str) -> String {
    let title_key = canonical_title(title);
    if title_key.is_empty() {
        format!("{}|{}", normalise_text(artist), normalise_text(title))
    } else {
        title_key
    }
}

fn canonical_title(title: &str) -> String {
    let mut out = title.to_lowercase();
    for sep in ['(', '[', '{'] {
        if let Some((head, _)) = out.split_once(sep) {
            out = head.to_string();
        }
    }
    for token in [
        "official audio",
        "official music video",
        "official video",
        "visualizer",
        "lyrics",
        "lyric video",
        "prod.",
        "produced by",
        "feat.",
        "ft.",
    ] {
        out = out.replace(token, " ");
    }
    normalise_text(&out)
}

fn normalise_text(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn has_variant_token(title: &str) -> bool {
    let t = title.to_lowercase();
    [
        "remix",
        " edit",
        "sped up",
        "slowed",
        "reverb",
        "nightcore",
        "8d",
        "loop",
        "cover",
        "instrumental",
        "karaoke",
        "live",
    ]
    .iter()
    .any(|needle| t.contains(needle))
}

fn is_low_quality_variant(title: &str, seed: &SimilarToSeed) -> bool {
    !has_variant_token(&seed.title) && has_variant_token(title)
}

fn is_seed_reupload(c: &Candidate, seed: &SimilarToSeed) -> bool {
    if seed.title.trim().is_empty() || canonical_title(&c.title) != canonical_title(&seed.title) {
        return false;
    }
    let seed_artist = normalise_text(&seed.artist);
    if seed_artist.is_empty() {
        return true;
    }
    let artist = normalise_text(&c.artist);
    artist.contains(&seed_artist) || seed_artist.contains(&artist)
}

struct ResolvedAcrossProviders {
    spotify: Option<Track>,
    soundcloud: Option<Track>,
}

async fn resolve_candidate(
    providers: &[Arc<dyn Provider>],
    c: &Candidate,
) -> ResolvedAcrossProviders {
    let mut spotify = None;
    let mut soundcloud = c.soundcloud_track.clone();
    let q = Query {
        text: format!("{} {}", c.artist, c.title),
        limit: Some(8),
    };
    let mut futs = FuturesUnordered::new();
    for p in providers {
        if soundcloud.is_some() && p.id() == ProviderId::SoundCloud {
            continue;
        }
        let p = p.clone();
        let q = q.clone();
        let artist = c.artist.clone();
        let title = c.title.clone();
        futs.push(async move {
            let id = p.id();
            let res = p.search(&q).await;
            (id, res, artist, title)
        });
    }
    while let Some((id, res, artist, title)) = futs.next().await {
        if let Ok(results) = res
            && let Some(top) = pick_best_match(&results.tracks, &artist, &title)
        {
            match id {
                ProviderId::Spotify => spotify = Some(top),
                ProviderId::SoundCloud => soundcloud = Some(top),
                ProviderId::the hi-res provider | ProviderId::Local => {}
            }
        }
    }
    ResolvedAcrossProviders {
        spotify,
        soundcloud,
    }
}

fn pick_best_match(tracks: &[Track], artist: &str, title: &str) -> Option<Track> {
    let want_title = canonical_title(title);
    let want_artist = normalise_text(artist);
    tracks
        .iter()
        .find(|t| {
            canonical_title(&t.title) == want_title
                && (want_artist.is_empty()
                    || t.artists.iter().any(|a| {
                        let got = normalise_text(&a.name);
                        got.contains(&want_artist) || want_artist.contains(&got)
                    }))
        })
        .cloned()
}

pub use provider_api::TrackUri as DiscoveryTrackUri;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn seed() -> SimilarToSeed {
        SimilarToSeed {
            artist: "Snow Strippers".into(),
            title: "Drench".into(),
            mbid: None,
        }
    }

    fn candidate(title: &str, artist: &str, score: f32, source: &'static str) -> Candidate {
        Candidate {
            mbid: None,
            title: title.into(),
            artist: artist.into(),
            score,
            sources: vec![source],
            soundcloud_track: None,
            source_rank: 0,
        }
    }

    fn sc_candidate(title: &str, artist: &str) -> Candidate {
        let track = Track {
            uri: provider_api::TrackUri(format!("soundcloud:track:{title}")),
            provider: ProviderId::SoundCloud,
            title: title.into(),
            artists: vec![provider_api::ArtistRef {
                uri: provider_api::ArtistUri("soundcloud:user:1".into()),
                name: artist.into(),
            }],
            album: None,
            duration: Duration::from_secs(180),
            cover_url: None,
            mbid: None,
            added_at: None,
        };
        Candidate {
            mbid: None,
            title: title.into(),
            artist: artist.into(),
            score: 0.9,
            sources: vec!["SoundCloud"],
            soundcloud_track: Some(track),
            source_rank: 2,
        }
    }

    #[test]
    fn canonical_title_strips_common_upload_noise() {
        assert_eq!(canonical_title("Drench (Official Audio)"), "drench");
        assert_eq!(canonical_title("Drench [Visualizer]"), "drench");
        assert_eq!(canonical_title("Drench ft. Someone"), "drench someone");
    }

    #[test]
    fn filters_variants_unless_seed_is_variant() {
        let normal = seed();
        assert!(is_low_quality_variant("Drench slowed + reverb", &normal));

        let remix_seed = SimilarToSeed {
            title: "Drench remix".into(),
            ..normal
        };
        assert!(!is_low_quality_variant("Drench remix", &remix_seed));
    }

    #[test]
    fn detects_seed_reuploads_from_same_artist() {
        let c = candidate("Drench (upload)", "Snow Strippers", 0.8, "Last.fm");
        assert!(is_seed_reupload(&c, &seed()));

        let other = candidate("Drench", "Different Artist", 0.8, "Last.fm");
        assert!(!is_seed_reupload(&other, &seed()));
    }

    #[test]
    fn sc_candidate_is_not_overwritten_by_support_merge() {
        let mut sc = sc_candidate("Cult", "Producer A");
        let support = candidate("Cult", "Producer B", 0.99, "Last.fm");
        merge_candidate(&mut sc, support);

        assert_eq!(sc.title, "Cult");
        assert_eq!(sc.artist, "Producer A");
        assert_eq!(sc.source_rank, 2);
        assert!(sc.soundcloud_track.is_some());
        assert!(sc.sources.contains(&"Last.fm"));
    }

    #[test]
    fn support_candidates_dedupe_and_merge_sources() {
        let lb = candidate("Track (Official Audio)", "A", 0.5, "ListenBrainz");
        let lf = candidate("Track", "B", 0.6, "Last.fm");
        let support = support_candidates(vec![lb], vec![lf], &SimilarToSeed::from_input("Seed"));
        assert_eq!(support.len(), 1);
        let merged = support.values().next().unwrap();
        assert!(merged.sources.contains(&"ListenBrainz"));
        assert!(merged.sources.contains(&"Last.fm"));
    }
}
