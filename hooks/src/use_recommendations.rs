//! Home "For You" / Explore recommendations.
//!
//! Mirrors the parts of Aegis that worked well: a SoundCloud-native Explore
//! landing with made-for-you rows, Daily Mix cards, because-you-played radio
//! seeds, artist-upload rows, and SoundCloud chart shelves.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use config::AppConfig;
use dioxus::prelude::*;
use discovery::{DiscoveryEngine, SimilarToSeed};
use player::HistoryEntry;
use provider_api::{ArtistUri, Provider, Query, Track, TrackUri};
use provider_soundcloud::SoundCloudProvider;
use rand::Rng;
use rand::seq::{IndexedRandom, SliceRandom};
use serde::{Deserialize, Serialize};

use crate::UseLibrary;
use crate::taste::{play_weight, weighted_sample};
use crate::use_likes::LikedTrack;

const SHELF_LIMIT: usize = 14;
const MADE_FOR_YOU_SEEDS: usize = 10;
/// Of the made-for-you seeds, how many are "explore" picks drawn uniformly
/// from outside the heaviest-rotation artists. This is what keeps the row
/// from collapsing into whatever was played last.
const MADE_FOR_YOU_EXPLORE: usize = 3;
/// Related tracks fetched per seed for the aggregate row. Small on purpose:
/// the row interleaves seeds round-robin, so every seed contributes instead
/// of the first two flooding it.
const PER_SEED_RELATED: usize = 4;
const DAILY_MIX_COUNT: usize = 4;
const DAILY_MIX_LIMIT: usize = 18;
/// Daily-mix clusters are sampled from this many top-weighted artists, not
/// taken as a fixed top-4.
const MIX_CANDIDATE_POOL: usize = 12;
/// Artists ranked above this count as the "head" of the taste profile;
/// explore picks deliberately come from below it.
const HEAD_ARTISTS: usize = 5;
/// Base weight of a like relative to a just-played track (1.0). Likes are
/// timeless but shouldn't outshout actual listening.
const LIKE_WEIGHT: f64 = 0.25;
/// SoundCloud-native seeds skip the search round-trip and hit the exact
/// related-tracks endpoint, so they're worth a bit more.
const SC_SEED_BONUS: f64 = 1.5;
/// Max tracks one artist contributes to the aggregate "Made for you" row.
const SHELF_ARTIST_CAP: usize = 2;
/// Max tracks one artist contributes to a single-seed row or daily mix —
/// those orbit one artist's neighbourhood, so a little repetition is fine.
const MIX_ARTIST_CAP: usize = 3;

const SHELF_MADE_FOR_YOU: &str = "made-for-you";
const SHELF_BECAUSE: &str = "because-recent";
const SHELF_NEW_FROM_ARTISTS: &str = "new-from-artists";
const SHELF_FROM_LIKES: &str = "from-likes";
const SHELF_TRENDING: &str = "trending-now";
const MIXES_GROUP: &str = "daily-mixes";

const GENRE_SHELVES: &[(&str, &str, &str)] = &[
    ("electronic", "Electronic", "electronic"),
    ("hiphoprap", "Hip-Hop / Rap", "hip hop rap"),
    ("house", "House", "house music"),
    ("techno", "Techno", "techno"),
    ("ambient", "Ambient", "ambient"),
    ("dnb", "Drum & Bass", "drum and bass"),
    ("indie", "Indie", "indie"),
];

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendationShelf {
    pub id: String,
    pub eyebrow: String,
    pub title: String,
    pub subtitle: String,
    pub seed_label: String,
    pub tracks: Vec<Track>,
    #[serde(skip, default)]
    pub is_loading: bool,
    #[serde(skip, default)]
    pub error: Option<String>,
    pub rerollable: bool,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendationMix {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub seed_label: String,
    pub tracks: Vec<Track>,
    #[serde(skip, default)]
    pub is_loading: bool,
    #[serde(skip, default)]
    pub error: Option<String>,
    pub accent_index: usize,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendationTile {
    pub id: String,
    pub label: String,
    pub sub: String,
    pub glyph: String,
    pub cover_url: Option<String>,
    pub tracks: Vec<Track>,
    pub accent_index: usize,
}

/// On-disk snapshot of the For-You dashboard. Loaded on Home mount so the
/// user sees their previous state immediately instead of skeletons while
/// the SoundCloud round-trips run.
#[derive(Serialize, Deserialize)]
struct RecommendationsCache {
    saved_at: DateTime<Utc>,
    shelves: Vec<RecommendationShelf>,
    mixes: Vec<RecommendationMix>,
    #[serde(default)]
    tiles: Vec<RecommendationTile>,
}

/// Cached shelves older than this trigger an auto-refresh on mount; cached
/// data still shows in the meantime, so the UX is "see prior dashboard +
/// quietly catch up", not "skeleton wipe".
const CACHE_TTL_HOURS: i64 = 24;

#[derive(Clone)]
pub struct UseRecommendations {
    pub shelves: Signal<Vec<RecommendationShelf>>,
    pub mixes: Signal<Vec<RecommendationMix>>,
    pub tiles: Signal<Vec<RecommendationTile>>,
    pub is_loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    engine: Arc<DiscoveryEngine>,
    sc: Arc<SoundCloudProvider>,
    history_entries: Signal<Vec<HistoryEntry>>,
    library_liked: Signal<Vec<Track>>,
    local_likes: Signal<Vec<LikedTrack>>,
    offsets: Signal<HashMap<String, usize>>,
    generation: Signal<u64>,
}

impl PartialEq for UseRecommendations {
    fn eq(&self, other: &Self) -> bool {
        self.shelves == other.shelves
            && self.mixes == other.mixes
            && self.tiles == other.tiles
            && self.is_loading == other.is_loading
            && self.error == other.error
            && self.offsets == other.offsets
            && self.generation == other.generation
    }
}

#[derive(Clone, Debug)]
struct RecommendationSeed {
    artist: String,
    title: String,
    label: String,
    track_uri: Option<TrackUri>,
}

#[derive(Clone)]
struct ShelfPlan {
    id: String,
    eyebrow: String,
    title: String,
    subtitle: String,
    seed_label: String,
    rerollable: bool,
    kind: ShelfKind,
}

#[derive(Clone)]
enum ShelfKind {
    RelatedAggregate {
        seeds: Vec<RecommendationSeed>,
        exclude_keys: HashSet<String>,
    },
    Related {
        seed: RecommendationSeed,
        exclude_keys: HashSet<String>,
    },
    ArtistUploads {
        artists: Vec<ArtistUri>,
    },
    StaticTracks {
        tracks: Vec<Track>,
    },
    Chart {
        genre: &'static str,
    },
    Search {
        query: &'static str,
    },
}

#[derive(Clone)]
struct MixPlan {
    id: String,
    title: String,
    subtitle: String,
    seed_label: String,
    seed: RecommendationSeed,
    accent_index: usize,
    exclude_keys: HashSet<String>,
}

impl UseRecommendations {
    pub fn refresh_all(&self) {
        self.load(None);
    }

    pub fn reroll_shelf(&self, id: String) {
        {
            let mut offsets = self.offsets;
            let mut map = offsets.peek().clone();
            *map.entry(id.clone()).or_insert(0) += 1;
            offsets.set(map);
        }
        self.load(Some(id));
    }

    fn load(&self, only: Option<String>) {
        let history = self.history_entries.read().clone();
        let spotify_liked = self.library_liked.read().clone();
        let local_likes = self.local_likes.read().clone();
        let offsets = self.offsets.read().clone();
        let liked_tracks = combined_likes(&local_likes, &spotify_liked);
        // Everything currently on the personalised rows counts as "already
        // shown": the next load down-ranks it so Refresh/Reroll actually
        // surfaces new tracks instead of replaying the same related-feed.
        // ponytail: one generation of memory (the visible dashboard); a
        // persisted ring buffer only if repeats still annoy in practice.
        let prev_keys = shown_track_keys(&self.shelves.peek(), &self.mixes.peek());
        let mut rng = rand::rng();
        let mut used_artists = HashSet::new();
        let shelf_plans = build_shelf_plans(
            &history,
            &liked_tracks,
            &prev_keys,
            &mut used_artists,
            &mut rng,
        );
        let mix_plans =
            build_mix_plans(&history, &liked_tracks, &prev_keys, &used_artists, &mut rng);

        let selected_shelves: Vec<ShelfPlan> = match only.as_ref() {
            Some(id) => shelf_plans.into_iter().filter(|p| &p.id == id).collect(),
            None => shelf_plans,
        };
        let selected_mixes: Vec<MixPlan> = match only.as_ref() {
            Some(id) if id == MIXES_GROUP => mix_plans,
            Some(id) => mix_plans.into_iter().filter(|p| &p.id == id).collect(),
            None => mix_plans,
        };

        if selected_shelves.is_empty() && selected_mixes.is_empty() {
            if self.shelves.peek().is_empty() && self.mixes.peek().is_empty() {
                let mut error = self.error;
                error.set(Some("Play or like a few tracks first.".into()));
            }
            return;
        }

        let load_generation = {
            let mut generation = self.generation;
            // Only a FULL refresh supersedes other loads. Selective rerolls
            // merge nothing but their own shelf, so overlapping rerolls of
            // different shelves are independent — sharing one generation
            // used to strand the first reroll in a permanent loading state
            // and throw its results away.
            if only.is_none() {
                let next = generation.peek().wrapping_add(1);
                generation.set(next);
            }
            *generation.peek()
        };

        // Selective rerolls of the SAME shelf can overlap (double-click on
        // Reroll); the shelf's offset counter is bumped before each load, so
        // it doubles as a per-shelf generation. A load only merges if the
        // offset it was started with is still current — otherwise the older
        // in-flight load would overwrite the newer one's results.
        let reroll_offset = only
            .as_ref()
            .map(|id| offsets.get(id).copied().unwrap_or(0));

        let mut shelves_sig = self.shelves;
        let mut mixes_sig = self.mixes;
        let mut tiles_sig = self.tiles;
        let mut loading_sig = self.is_loading;
        let mut error_sig = self.error;
        let generation_sig = self.generation;
        let offsets_sig = self.offsets;
        let engine = self.engine.clone();
        let sc = self.sc.clone();
        let only_id = only.clone();

        mark_shelves_loading(&mut shelves_sig, &selected_shelves, only_id.as_deref());
        mark_mixes_loading(&mut mixes_sig, &selected_mixes, only_id.as_deref());
        let marked_shelf_ids: Vec<String> =
            selected_shelves.iter().map(|p| p.id.clone()).collect();
        let marked_mix_ids: Vec<String> = selected_mixes.iter().map(|p| p.id.clone()).collect();

        spawn(async move {
            loading_sig.set(true);
            error_sig.set(None);

            let mut loaded_shelves = Vec::<RecommendationShelf>::new();
            for plan in selected_shelves {
                loaded_shelves.push(load_shelf(sc.clone(), engine.clone(), plan).await);
            }

            let mut loaded_mixes = Vec::<RecommendationMix>::new();
            for plan in selected_mixes {
                loaded_mixes.push(load_mix(sc.clone(), engine.clone(), plan).await);
            }
            dedupe_across_mixes(&mut loaded_mixes);

            if *generation_sig.peek() != load_generation {
                // A full refresh superseded this load. Drop the results, but
                // don't strand the rows we marked as loading — the refresh
                // repaints them, and a stuck flag disables their buttons.
                clear_shelf_loading_flags(&mut shelves_sig, &marked_shelf_ids);
                clear_mix_loading_flags(&mut mixes_sig, &marked_mix_ids);
                return;
            }
            if let (Some(id), Some(started_offset)) = (only_id.as_deref(), reroll_offset)
                && offsets_sig.peek().get(id).copied().unwrap_or(0) != started_offset
            {
                // A newer reroll of this shelf superseded us. It re-marked the
                // loading flags and will merge its own results — just drop ours.
                return;
            }

            let had_error = loaded_shelves
                .iter()
                .find_map(|s| s.error.clone())
                .or_else(|| loaded_mixes.iter().find_map(|m| m.error.clone()));
            let has_loaded_tracks = loaded_shelves.iter().any(|s| !s.tracks.is_empty())
                || loaded_mixes.iter().any(|m| !m.tracks.is_empty());
            merge_loaded_shelves(&mut shelves_sig, loaded_shelves, only_id.as_deref());
            merge_loaded_mixes(&mut mixes_sig, loaded_mixes, only_id.as_deref());
            let shelves_now = shelves_sig.peek().clone();
            let mixes_now = mixes_sig.peek().clone();
            let tiles_now = build_tiles(&shelves_now, &mixes_now);
            tiles_sig.set(tiles_now.clone());
            error_sig.set(if has_loaded_tracks { None } else { had_error });
            loading_sig.set(false);

            // Persist the dashboard so the next cold-start shows this state
            // instantly. Best-effort — a cache write failure must not break
            // playback or surface to the user.
            if has_loaded_tracks
                && let Err(e) = save_cache(&RecommendationsCache {
                    saved_at: Utc::now(),
                    shelves: shelves_now,
                    mixes: mixes_now,
                    tiles: tiles_now,
                })
            {
                tracing::debug!(error = %e, "recommendations: cache save failed");
            }
        });
    }
}

pub fn use_recommendations(
    library: UseLibrary,
    history_entries: Signal<Vec<HistoryEntry>>,
) -> UseRecommendations {
    let engine = use_context::<Arc<DiscoveryEngine>>();
    let sc = use_context::<Arc<SoundCloudProvider>>();
    let local_likes = crate::use_likes::use_likes();
    let mut shelves = use_signal(Vec::<RecommendationShelf>::new);
    let mut mixes = use_signal(Vec::<RecommendationMix>::new);
    let mut tiles = use_signal(Vec::<RecommendationTile>::new);
    let is_loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let offsets = use_signal(HashMap::<String, usize>::new);
    let generation = use_signal(|| 0u64);
    let mut cache_fresh = use_signal(|| false);
    // One-shot gate so the 30-second history tick (and any other signal
    // update inside the effect's dep set) can't retrigger a background
    // refresh after the initial mount decision. Manual Refresh / Reroll
    // bypass this since they go through methods, not the effect.
    let mut did_initial_load = use_signal(|| false);

    // Hydrate from disk once on mount so Home shows the previous dashboard
    // before the SoundCloud round-trips finish. Stale cache still renders;
    // the effect below will quietly auto-refresh when older than the TTL.
    use_hook(move || {
        if let Some(cache) = load_cache() {
            let fresh = (Utc::now() - cache.saved_at) < chrono::Duration::hours(CACHE_TTL_HOURS);
            shelves.set(cache.shelves);
            mixes.set(cache.mixes);
            tiles.set(cache.tiles);
            cache_fresh.set(fresh);
        }
    });

    let handle = UseRecommendations {
        shelves,
        mixes,
        tiles,
        is_loading,
        error,
        engine,
        sc,
        history_entries,
        library_liked: library.liked,
        local_likes: local_likes.items,
        offsets,
        generation,
    };

    {
        let handle = handle.clone();
        use_effect(move || {
            let history_len = history_entries.read().len();
            let spotify_len = library.liked.read().len();
            let local_len = local_likes.items.read().len();
            let in_flight = *handle.is_loading.peek();
            if *did_initial_load.peek() || in_flight {
                return;
            }
            // Wait until at least one user-data source has populated, then
            // commit to a single mount-time decision and never auto-refresh
            // again — the user can hit Refresh / Reroll explicitly.
            if history_len + spotify_len + local_len == 0 {
                return;
            }
            did_initial_load.set(true);
            let has_content = !handle.shelves.peek().is_empty() || !handle.mixes.peek().is_empty();
            // Fresh cache already populated the signals — show the previous
            // dashboard without a network refresh.
            if has_content && *cache_fresh.peek() {
                return;
            }
            handle.refresh_all();
        });
    }

    handle
}

fn load_cache() -> Option<RecommendationsCache> {
    let path = AppConfig::recommendations_cache_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_cache(cache: &RecommendationsCache) -> anyhow::Result<()> {
    let Some(path) = AppConfig::recommendations_cache_path() else {
        return Ok(());
    };
    AppConfig::atomic_write_json(&path, cache)
}

async fn load_shelf(
    sc: Arc<SoundCloudProvider>,
    engine: Arc<DiscoveryEngine>,
    plan: ShelfPlan,
) -> RecommendationShelf {
    let result = match plan.kind.clone() {
        ShelfKind::RelatedAggregate {
            seeds,
            exclude_keys,
        } => related_aggregate(sc, engine, &seeds, &exclude_keys, SHELF_LIMIT).await,
        ShelfKind::Related { seed, exclude_keys } => {
            related_for_seed(sc, engine, &seed, SHELF_LIMIT * 2)
                .await
                .map(|tracks| curate_tracks(tracks, &exclude_keys, MIX_ARTIST_CAP, SHELF_LIMIT))
        }
        ShelfKind::ArtistUploads { artists } => artist_uploads(sc, &artists, SHELF_LIMIT).await,
        ShelfKind::StaticTracks { tracks } => Ok(dedupe_tracks(tracks)
            .into_iter()
            .take(SHELF_LIMIT)
            .collect()),
        ShelfKind::Chart { genre } => sc
            .genre_chart(genre, SHELF_LIMIT as u32)
            .await
            .map_err(|e| e.to_string()),
        ShelfKind::Search { query } => sc
            .search(&Query {
                text: query.to_string(),
                limit: Some(SHELF_LIMIT as u32),
            })
            .await
            .map(|r| r.tracks)
            .map_err(|e| e.to_string()),
    };

    match result {
        Ok(tracks) => RecommendationShelf {
            id: plan.id,
            eyebrow: plan.eyebrow,
            title: plan.title,
            subtitle: plan.subtitle,
            seed_label: plan.seed_label,
            tracks: dedupe_tracks(tracks)
                .into_iter()
                .take(SHELF_LIMIT)
                .collect(),
            is_loading: false,
            error: None,
            rerollable: plan.rerollable,
        },
        Err(e) => RecommendationShelf {
            id: plan.id,
            eyebrow: plan.eyebrow,
            title: plan.title,
            subtitle: plan.subtitle,
            seed_label: plan.seed_label,
            tracks: Vec::new(),
            is_loading: false,
            error: Some(e),
            rerollable: plan.rerollable,
        },
    }
}

async fn load_mix(
    sc: Arc<SoundCloudProvider>,
    engine: Arc<DiscoveryEngine>,
    plan: MixPlan,
) -> RecommendationMix {
    // Fetch double so the exclusion filter (likes + already-shown) still
    // leaves a full mix.
    let result = related_for_seed(sc, engine, &plan.seed, DAILY_MIX_LIMIT * 2)
        .await
        .map(|tracks| curate_tracks(tracks, &plan.exclude_keys, MIX_ARTIST_CAP, DAILY_MIX_LIMIT));

    match result {
        Ok(tracks) => RecommendationMix {
            id: plan.id,
            title: plan.title,
            subtitle: plan.subtitle,
            seed_label: plan.seed_label,
            tracks: dedupe_tracks(tracks)
                .into_iter()
                .take(DAILY_MIX_LIMIT)
                .collect(),
            is_loading: false,
            error: None,
            accent_index: plan.accent_index,
        },
        Err(e) => RecommendationMix {
            id: plan.id,
            title: plan.title,
            subtitle: plan.subtitle,
            seed_label: plan.seed_label,
            tracks: Vec::new(),
            is_loading: false,
            error: Some(e),
            accent_index: plan.accent_index,
        },
    }
}

/// Collapse same-song duplicates (different uploader, cover, other platform)
/// by noise-stripped title. Only used on recommendation feeds — a user's own
/// likes keep deliberate duplicates.
fn dedupe_canonical(tracks: Vec<Track>) -> Vec<Track> {
    let mut seen = HashSet::<String>::new();
    tracks
        .into_iter()
        .filter(|t| {
            let canon = discovery::canonical_title(&t.title);
            canon.is_empty() || seen.insert(canon)
        })
        .collect()
}

/// Curate a raw related feed into a row: dedupe (exact + canonical title),
/// drop excluded tracks, cap how many tracks one artist contributes. Never
/// starves the row — if filtering leaves too little, dropped tracks are
/// backfilled rather than showing a gap.
fn curate_tracks(
    tracks: Vec<Track>,
    exclude_keys: &HashSet<String>,
    max_per_artist: usize,
    limit: usize,
) -> Vec<Track> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut kept = Vec::new();
    let mut spare = Vec::new();
    for track in dedupe_canonical(dedupe_tracks(tracks)) {
        if exclude_keys.contains(&track_key(&track)) {
            spare.push(track);
            continue;
        }
        let artist = normalise_key(
            track
                .artists
                .first()
                .map(|a| a.name.as_str())
                .unwrap_or_default(),
        );
        let count = counts.entry(artist).or_insert(0);
        if *count < max_per_artist {
            *count += 1;
            kept.push(track);
        } else {
            spare.push(track);
        }
    }
    if kept.len() < limit.div_ceil(2) {
        kept.extend(spare);
    }
    kept.into_iter().take(limit).collect()
}

/// Fetch related tracks for every seed in parallel, then interleave them
/// round-robin. The old sequential fill let the first two seeds flood the
/// whole row — which is exactly why two plays of one artist used to turn
/// "Made for you" into that artist's radio.
async fn related_aggregate(
    sc: Arc<SoundCloudProvider>,
    engine: Arc<DiscoveryEngine>,
    seeds: &[RecommendationSeed],
    exclude_keys: &HashSet<String>,
    limit: usize,
) -> Result<Vec<Track>, String> {
    let fetches = seeds.iter().take(MADE_FOR_YOU_SEEDS).map(|seed| {
        related_for_seed(sc.clone(), engine.clone(), seed, PER_SEED_RELATED)
    });
    let results = futures_util::future::join_all(fetches).await;
    let mut last_err = None;
    let mut pools: Vec<std::vec::IntoIter<Track>> = Vec::new();
    for r in results {
        match r {
            Ok(tracks) => pools.push(tracks.into_iter()),
            Err(e) => last_err = Some(e),
        }
    }
    if pools.is_empty() {
        return Err(last_err.unwrap_or_else(|| "no seeds to aggregate".into()));
    }

    let mut seen_uri = HashSet::new();
    let mut seen_key = HashSet::new();
    let mut seen_canon = HashSet::new();
    let mut artist_counts: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    let mut spare = Vec::new();
    'rounds: loop {
        let mut any = false;
        for pool in &mut pools {
            let Some(track) = pool.next() else { continue };
            any = true;
            let key = track_key(&track);
            if !seen_uri.insert(track.uri.0.clone()) || !seen_key.insert(key.clone()) {
                continue;
            }
            // Same song under a different uploader/cover: hard-drop, never
            // even as backfill — ten copies of one track is not a mix.
            let canon = discovery::canonical_title(&track.title);
            if !canon.is_empty() && !seen_canon.insert(canon) {
                continue;
            }
            if exclude_keys.contains(&key) {
                spare.push(track);
                continue;
            }
            let artist = normalise_key(
                track
                    .artists
                    .first()
                    .map(|a| a.name.as_str())
                    .unwrap_or_default(),
            );
            let count = artist_counts.entry(artist).or_insert(0);
            if *count >= SHELF_ARTIST_CAP {
                spare.push(track);
                continue;
            }
            *count += 1;
            out.push(track);
            if out.len() >= limit {
                break 'rounds;
            }
        }
        if !any {
            break;
        }
    }
    // Short row (niche seeds, heavy exclusion): pad with excluded tracks
    // rather than rendering half a shelf.
    for track in spare {
        if out.len() >= limit {
            break;
        }
        out.push(track);
    }
    Ok(out)
}

async fn related_for_seed(
    sc: Arc<SoundCloudProvider>,
    engine: Arc<DiscoveryEngine>,
    seed: &RecommendationSeed,
    limit: usize,
) -> Result<Vec<Track>, String> {
    if let Some(uri) = seed
        .track_uri
        .as_ref()
        .filter(|uri| is_soundcloud_track(uri))
    {
        // Fetch double: the variant/reupload filter and canonical dedupe eat
        // into the raw feed. SC caps the endpoint at ~50.
        let fetch = (limit * 2).min(50) as u32;
        return sc
            .related_tracks(uri, fetch)
            .await
            .map(|tracks| {
                dedupe_canonical(dedupe_tracks(filter_seed_variants(tracks, seed)))
                    .into_iter()
                    .take(limit)
                    .collect()
            })
            .map_err(|e| e.to_string());
    }

    let seed_input = SimilarToSeed {
        artist: seed.artist.clone(),
        title: seed.title.clone(),
        mbid: None,
    };
    engine
        .similar_to(seed_input)
        .await
        .map(|results| {
            dedupe_tracks(
                results
                    .into_iter()
                    .filter_map(|r| r.play_target())
                    .take(limit * 2)
                    .collect(),
            )
            .into_iter()
            .take(limit)
            .collect()
        })
        .map_err(|e| e.to_string())
}

/// Hygiene for raw SoundCloud related feeds, mirroring what the discovery
/// engine does on its own paths: drop reuploads/covers of the seed itself
/// and low-quality variants (sped up, nightcore, …) the seed isn't one of.
fn filter_seed_variants(tracks: Vec<Track>, seed: &RecommendationSeed) -> Vec<Track> {
    let seed_canon = discovery::canonical_title(&seed.title);
    let seed_ref = SimilarToSeed {
        artist: seed.artist.clone(),
        title: seed.title.clone(),
        mbid: None,
    };
    tracks
        .into_iter()
        .filter(|t| {
            let canon = discovery::canonical_title(&t.title);
            (canon.is_empty() || canon != seed_canon)
                && !discovery::is_low_quality_variant(&t.title, &seed_ref)
        })
        .collect()
}

async fn artist_uploads(
    sc: Arc<SoundCloudProvider>,
    artists: &[ArtistUri],
    limit: usize,
) -> Result<Vec<Track>, String> {
    let mut out = Vec::new();
    for artist in artists.iter().take(8) {
        match sc.user_tracks(artist, 3).await {
            Ok(tracks) => out.extend(tracks),
            Err(e) => {
                tracing::debug!(artist = %artist.0, error = %e, "new-from-artists seed failed")
            }
        }
        if out.len() >= limit * 2 {
            break;
        }
    }
    Ok(dedupe_tracks(out).into_iter().take(limit).collect())
}

fn mark_shelves_loading(
    shelves_sig: &mut Signal<Vec<RecommendationShelf>>,
    plans: &[ShelfPlan],
    _only_id: Option<&str>,
) {
    let mut shelves = shelves_sig.peek().clone();
    if shelves.is_empty() {
        // Cold start: render skeletons so the user sees the row scaffold.
        shelves = plans.iter().map(skeleton_shelf).collect();
    } else {
        // Refresh / reroll with cached content visible: keep the tracks but
        // mark the rows loading so the UI can show a subtle refresh state.
        for plan in plans {
            if let Some(existing) = shelves.iter_mut().find(|s| s.id == plan.id) {
                existing.is_loading = true;
                existing.error = None;
                existing.seed_label = plan.seed_label.clone();
                existing.subtitle = plan.subtitle.clone();
            } else {
                shelves.push(skeleton_shelf(plan));
            }
        }
    }
    shelves_sig.set(shelves);
}

fn mark_mixes_loading(
    mixes_sig: &mut Signal<Vec<RecommendationMix>>,
    plans: &[MixPlan],
    _only_id: Option<&str>,
) {
    let mut mixes = mixes_sig.peek().clone();
    if mixes.is_empty() {
        mixes = plans.iter().map(skeleton_mix).collect();
    } else {
        for plan in plans {
            if let Some(existing) = mixes.iter_mut().find(|m| m.id == plan.id) {
                existing.is_loading = true;
                existing.error = None;
                existing.seed_label = plan.seed_label.clone();
                existing.subtitle = plan.subtitle.clone();
            } else {
                mixes.push(skeleton_mix(plan));
            }
        }
    }
    mixes_sig.set(mixes);
}

fn merge_loaded_shelves(
    shelves_sig: &mut Signal<Vec<RecommendationShelf>>,
    loaded: Vec<RecommendationShelf>,
    only_id: Option<&str>,
) {
    // Failed load with cached content on screen: keep showing the previous
    // good tracks instead of blanking the row — the error still lands in
    // `shelf.error` so the UI can surface it. Without this, hitting Refresh
    // while offline wiped the whole visible dashboard into error rows.
    let prev = shelves_sig.peek().clone();
    let keep_prev_tracks = |mut shelf: RecommendationShelf| {
        if shelf.tracks.is_empty()
            && shelf.error.is_some()
            && let Some(old) = prev.iter().find(|s| s.id == shelf.id)
            && !old.tracks.is_empty()
        {
            shelf.tracks = old.tracks.clone();
        }
        shelf
    };
    if only_id.is_none() {
        shelves_sig.set(loaded.into_iter().map(keep_prev_tracks).collect());
        return;
    }
    let mut shelves = shelves_sig.peek().clone();
    for shelf in loaded {
        let shelf = keep_prev_tracks(shelf);
        if let Some(existing) = shelves.iter_mut().find(|s| s.id == shelf.id) {
            *existing = shelf;
        } else {
            shelves.push(shelf);
        }
    }
    shelves_sig.set(shelves);
}

fn merge_loaded_mixes(
    mixes_sig: &mut Signal<Vec<RecommendationMix>>,
    loaded: Vec<RecommendationMix>,
    only_id: Option<&str>,
) {
    // Same failed-refresh guard as merge_loaded_shelves.
    let prev = mixes_sig.peek().clone();
    let keep_prev_tracks = |mut mix: RecommendationMix| {
        if mix.tracks.is_empty()
            && mix.error.is_some()
            && let Some(old) = prev.iter().find(|m| m.id == mix.id)
            && !old.tracks.is_empty()
        {
            mix.tracks = old.tracks.clone();
        }
        mix
    };
    if only_id.is_none() {
        mixes_sig.set(loaded.into_iter().map(keep_prev_tracks).collect());
        return;
    }
    let mut mixes = mixes_sig.peek().clone();
    for mix in loaded {
        let mix = keep_prev_tracks(mix);
        if let Some(existing) = mixes.iter_mut().find(|m| m.id == mix.id) {
            *existing = mix;
        } else {
            mixes.push(mix);
        }
    }
    mixes_sig.set(mixes);
}

/// Reset `is_loading` on the given shelf ids — used when a superseded load
/// bails out so its rows don't stay stuck in a loading state.
fn clear_shelf_loading_flags(sig: &mut Signal<Vec<RecommendationShelf>>, ids: &[String]) {
    let mut shelves = sig.peek().clone();
    let mut changed = false;
    for shelf in shelves.iter_mut() {
        if shelf.is_loading && ids.contains(&shelf.id) {
            shelf.is_loading = false;
            changed = true;
        }
    }
    if changed {
        sig.set(shelves);
    }
}

fn clear_mix_loading_flags(sig: &mut Signal<Vec<RecommendationMix>>, ids: &[String]) {
    let mut mixes = sig.peek().clone();
    let mut changed = false;
    for mix in mixes.iter_mut() {
        if mix.is_loading && ids.contains(&mix.id) {
            mix.is_loading = false;
            changed = true;
        }
    }
    if changed {
        sig.set(mixes);
    }
}

fn skeleton_shelf(plan: &ShelfPlan) -> RecommendationShelf {
    RecommendationShelf {
        id: plan.id.clone(),
        eyebrow: plan.eyebrow.clone(),
        title: plan.title.clone(),
        subtitle: plan.subtitle.clone(),
        seed_label: plan.seed_label.clone(),
        tracks: Vec::new(),
        is_loading: true,
        error: None,
        rerollable: plan.rerollable,
    }
}

fn skeleton_mix(plan: &MixPlan) -> RecommendationMix {
    RecommendationMix {
        id: plan.id.clone(),
        title: plan.title.clone(),
        subtitle: plan.subtitle.clone(),
        seed_label: plan.seed_label.clone(),
        tracks: Vec::new(),
        is_loading: true,
        error: None,
        accent_index: plan.accent_index,
    }
}

fn build_shelf_plans(
    history: &[HistoryEntry],
    liked: &[Track],
    prev_keys: &HashSet<String>,
    used_artists: &mut HashSet<String>,
    rng: &mut impl Rng,
) -> Vec<ShelfPlan> {
    let mut out = Vec::new();
    let now = Utc::now();
    let liked_keys = liked.iter().map(track_key).collect::<HashSet<_>>();
    let exclude_keys: HashSet<String> = liked_keys.union(prev_keys).cloned().collect();
    let pool = weighted_seed_pool(history, liked, now);

    if !pool.is_empty() {
        // Weighted random seeds (recency-decayed, likes included) plus a few
        // explore picks from the long tail; max two seeds per artist.
        let made_for_you_seeds =
            sample_seeds_with_explore(&pool, MADE_FOR_YOU_SEEDS, MADE_FOR_YOU_EXPLORE, rng);
        out.push(ShelfPlan {
            id: SHELF_MADE_FOR_YOU.to_string(),
            eyebrow: "Personal".into(),
            title: "Made for you".into(),
            subtitle: "Aegis-style SoundCloud related picks from your recent plays and likes."
                .into(),
            seed_label: "recent plays + likes".into(),
            rerollable: true,
            kind: ShelfKind::RelatedAggregate {
                seeds: made_for_you_seeds,
                exclude_keys: exclude_keys.clone(),
            },
        });
    }

    // A weighted draw over the recent plays — usually something fresh, but
    // not tautologically "the last track you touched".
    let recent_pool = recent_play_pool(history, now, 30);
    if let Some(seed) = weighted_sample(recent_pool, 1, rng).into_iter().next() {
        used_artists.insert(normalise_key(&seed.artist));
        out.push(ShelfPlan {
            id: SHELF_BECAUSE.to_string(),
            eyebrow: "Because you played".into(),
            title: format!("\"{}\"", seed.title),
            subtitle: format!("by {}", seed.artist),
            seed_label: seed.label.clone(),
            rerollable: true,
            kind: ShelfKind::Related {
                seed,
                exclude_keys: exclude_keys.clone(),
            },
        });
    }

    let mut artist_uris = soundcloud_artist_uris(liked);
    if !artist_uris.is_empty() {
        let count = artist_uris.len();
        artist_uris.shuffle(rng);
        out.push(ShelfPlan {
            id: SHELF_NEW_FROM_ARTISTS.to_string(),
            eyebrow: "New releases".into(),
            title: "From your artists".into(),
            subtitle: "Recent uploads from SoundCloud artists you saved.".into(),
            seed_label: format!("{count} SoundCloud artists"),
            rerollable: true,
            kind: ShelfKind::ArtistUploads {
                artists: artist_uris,
            },
        });
    }

    if !liked.is_empty() {
        out.push(ShelfPlan {
            id: SHELF_FROM_LIKES.to_string(),
            eyebrow: "Rediscover".into(),
            title: "From your likes".into(),
            subtitle: "A rotating slice of what you saved.".into(),
            seed_label: format!("{} saved tracks", liked.len()),
            rerollable: true,
            kind: ShelfKind::StaticTracks {
                tracks: liked.choose_multiple(rng, SHELF_LIMIT).cloned().collect(),
            },
        });
    }

    out.push(ShelfPlan {
        id: SHELF_TRENDING.to_string(),
        eyebrow: "SoundCloud".into(),
        title: "Trending now".into(),
        subtitle: "What is moving across SoundCloud right now.".into(),
        seed_label: "SoundCloud charts".into(),
        rerollable: true,
        kind: ShelfKind::Chart { genre: "all-music" },
    });

    for (slug, label, query) in GENRE_SHELVES {
        out.push(ShelfPlan {
            id: format!("genre-{slug}"),
            eyebrow: "Genre".into(),
            title: (*label).into(),
            subtitle: "Fresh SoundCloud search picks, tuned as scene rows.".into(),
            seed_label: format!("SoundCloud / {label}"),
            rerollable: true,
            kind: ShelfKind::Search { query },
        });
    }

    out
}

fn build_mix_plans(
    history: &[HistoryEntry],
    liked: &[Track],
    prev_keys: &HashSet<String>,
    used_artists: &HashSet<String>,
    rng: &mut impl Rng,
) -> Vec<MixPlan> {
    let now = Utc::now();
    let mut clusters = cluster_seeds(history, liked, now);
    // Keep the Because-you-played artist from also fronting a mix — unless
    // the profile is so small that dropping it would leave mixes empty.
    if clusters.len() > DAILY_MIX_COUNT {
        let filtered: Vec<_> = clusters
            .iter()
            .filter(|(_, s)| !used_artists.contains(&normalise_key(&s.artist)))
            .cloned()
            .collect();
        if filtered.len() >= DAILY_MIX_COUNT {
            clusters = filtered;
        }
    }
    if clusters.is_empty() {
        return Vec::new();
    }
    let liked_keys = liked.iter().map(track_key).collect::<HashSet<_>>();
    let exclude_keys: HashSet<String> = liked_keys.union(prev_keys).cloned().collect();

    // Three weighted draws from the profile's head, one uniform explore draw
    // from beyond it — one mix is always a "you forgot about this" lane.
    // Clusters are one-per-artist, so sampling without replacement already
    // guarantees four distinct artists.
    let head: Vec<(f64, RecommendationSeed)> = clusters
        .iter()
        .take(MIX_CANDIDATE_POOL)
        .cloned()
        .collect();
    let mut picked = weighted_sample(head, DAILY_MIX_COUNT - 1, rng);
    let picked_artists: HashSet<String> =
        picked.iter().map(|s| normalise_key(&s.artist)).collect();
    let tail: Vec<(f64, RecommendationSeed)> = clusters
        .iter()
        .skip(HEAD_ARTISTS)
        .filter(|(_, s)| !picked_artists.contains(&normalise_key(&s.artist)))
        .map(|(_, s)| (1.0, s.clone()))
        .collect();
    picked.extend(weighted_sample(tail, 1, rng));
    if picked.len() < DAILY_MIX_COUNT {
        // Small profile: top up from whatever clusters remain.
        let remaining: Vec<(f64, RecommendationSeed)> = clusters
            .iter()
            .filter(|(_, s)| {
                let key = normalise_key(&s.artist);
                picked.iter().all(|p| normalise_key(&p.artist) != key)
            })
            .cloned()
            .collect();
        let need = DAILY_MIX_COUNT - picked.len();
        picked.extend(weighted_sample(remaining, need, rng));
    }

    picked
        .into_iter()
        .take(DAILY_MIX_COUNT)
        .enumerate()
        .map(|(slot, seed)| MixPlan {
            id: format!("daily-mix-{slot}"),
            title: format!("Daily Mix {}", slot + 1),
            subtitle: format!("{}, related scenes and more", seed.artist),
            seed_label: seed.label.clone(),
            seed,
            accent_index: slot,
            exclude_keys: exclude_keys.clone(),
        })
        .collect()
}

fn build_tiles(
    shelves: &[RecommendationShelf],
    mixes: &[RecommendationMix],
) -> Vec<RecommendationTile> {
    let mut out = Vec::new();

    if let Some(shelf) = filled_shelf(shelves, SHELF_MADE_FOR_YOU) {
        out.push(tile_from_tracks(
            "made-for-you",
            "Made for you",
            "SoundCloud picks",
            "✦",
            &shelf.tracks,
            2,
        ));
    }
    if let Some(mix) = mixes.iter().find(|m| !m.tracks.is_empty()) {
        out.push(tile_from_tracks(
            "daily-mix-1",
            &mix.title,
            &mix.seed_label,
            "◎",
            &mix.tracks,
            4,
        ));
    }
    if let Some(shelf) = filled_shelf(shelves, SHELF_FROM_LIKES) {
        out.push(tile_from_tracks(
            "from-likes",
            "From your likes",
            &format!("{} tracks", shelf.tracks.len()),
            "★",
            &shelf.tracks,
            6,
        ));
    }
    if let Some(shelf) = filled_shelf(shelves, SHELF_TRENDING) {
        out.push(tile_from_tracks(
            "trending",
            "Trending",
            "SoundCloud charts",
            "⚡",
            &shelf.tracks,
            1,
        ));
    }

    out
}

fn filled_shelf<'a>(
    shelves: &'a [RecommendationShelf],
    id: &str,
) -> Option<&'a RecommendationShelf> {
    shelves.iter().find(|s| s.id == id && !s.tracks.is_empty())
}

fn tile_from_tracks(
    id: &str,
    label: &str,
    sub: &str,
    glyph: &str,
    tracks: &[Track],
    accent_index: usize,
) -> RecommendationTile {
    RecommendationTile {
        id: id.into(),
        label: label.into(),
        sub: sub.into(),
        glyph: glyph.into(),
        cover_url: tracks.first().and_then(|t| t.cover_url.clone()),
        tracks: tracks.to_vec(),
        accent_index,
    }
}

fn combined_likes(local: &[LikedTrack], spotify: &[Track]) -> Vec<Track> {
    let mut tracks = local.iter().map(|l| l.track.clone()).collect::<Vec<_>>();
    tracks.extend(spotify.iter().cloned());
    dedupe_tracks(tracks)
}

/// A seed with its taste-profile weight: decayed play mass plus like mass.
#[derive(Clone, Debug)]
struct WeightedSeed {
    weight: f64,
    seed: RecommendationSeed,
}

fn seed_key(seed: &RecommendationSeed) -> String {
    format!(
        "{}|{}",
        normalise_key(&seed.artist),
        normalise_key(&seed.title)
    )
}

fn sc_seed_bonus(seed: &RecommendationSeed) -> f64 {
    if seed.track_uri.as_ref().is_some_and(is_soundcloud_track) {
        SC_SEED_BONUS
    } else {
        1.0
    }
}

/// Every known seed with an accumulated weight: each play adds its decayed
/// weight, each like adds a flat base. History runs first so a seed's URI
/// identity comes from the most recent play, not an old like.
fn weighted_seed_pool(
    history: &[HistoryEntry],
    liked: &[Track],
    now: DateTime<Utc>,
) -> Vec<WeightedSeed> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<WeightedSeed> = Vec::new();
    let mut upsert = |seed: RecommendationSeed, weight: f64| match index.get(&seed_key(&seed)) {
        Some(&i) => out[i].weight += weight,
        None => {
            index.insert(seed_key(&seed), out.len());
            out.push(WeightedSeed { weight, seed });
        }
    };
    for e in history {
        let Some(seed) = seed_from_parts(&e.artist, &e.title, e.track_uri.clone().map(TrackUri))
        else {
            continue;
        };
        let weight = play_weight(now, e.played_at) * sc_seed_bonus(&seed);
        upsert(seed, weight);
    }
    for track in liked {
        let Some(seed) = seed_from_track(track) else {
            continue;
        };
        let weight = LIKE_WEIGHT * sc_seed_bonus(&seed);
        upsert(seed, weight);
    }
    out
}

/// The most recent plays, deduped, each weighted by decay — the pool the
/// "Because you played" seed is drawn from.
fn recent_play_pool(
    history: &[HistoryEntry],
    now: DateTime<Utc>,
    cap: usize,
) -> Vec<(f64, RecommendationSeed)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in history {
        let Some(seed) = seed_from_parts(&e.artist, &e.title, e.track_uri.clone().map(TrackUri))
        else {
            continue;
        };
        if !seen.insert(seed_key(&seed)) {
            continue;
        }
        out.push((play_weight(now, e.played_at), seed));
        if out.len() >= cap {
            break;
        }
    }
    out
}

/// Draw `n` seeds from the pool: `n - explore` weighted by the taste
/// profile, plus `explore` uniform picks from outside the top artists.
/// The per-artist cap of 2 still applies to the combined result.
fn sample_seeds_with_explore(
    pool: &[WeightedSeed],
    n: usize,
    explore: usize,
    rng: &mut impl Rng,
) -> Vec<RecommendationSeed> {
    let mut by_artist: HashMap<String, f64> = HashMap::new();
    for ws in pool {
        *by_artist
            .entry(normalise_key(&ws.seed.artist))
            .or_default() += ws.weight;
    }
    let mut ranked: Vec<(String, f64)> = by_artist.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let head: HashSet<String> = ranked
        .into_iter()
        .take(HEAD_ARTISTS)
        .map(|(artist, _)| artist)
        .collect();

    let exploit = weighted_sample(
        pool.iter().map(|ws| (ws.weight, ws.seed.clone())).collect(),
        n.saturating_sub(explore),
        rng,
    );
    let picked: HashSet<String> = exploit.iter().map(seed_key).collect();
    let tail: Vec<(f64, RecommendationSeed)> = pool
        .iter()
        .filter(|ws| !head.contains(&normalise_key(&ws.seed.artist)))
        .filter(|ws| !picked.contains(&seed_key(&ws.seed)))
        .map(|ws| (1.0, ws.seed.clone()))
        .collect();
    let mut seeds = exploit;
    seeds.extend(weighted_sample(tail, explore, rng));
    diverse_seeds(seeds, 2).into_iter().take(n).collect()
}

/// Track keys of everything the personalised rows currently display, so the
/// next load can avoid repeating them. Chart/genre rows are left out — they
/// show what SoundCloud serves, not what we picked.
fn shown_track_keys(
    shelves: &[RecommendationShelf],
    mixes: &[RecommendationMix],
) -> HashSet<String> {
    shelves
        .iter()
        .filter(|s| s.id == SHELF_MADE_FOR_YOU || s.id == SHELF_BECAUSE)
        .flat_map(|s| s.tracks.iter().map(track_key))
        .chain(mixes.iter().flat_map(|m| m.tracks.iter().map(track_key)))
        .collect()
}

/// One cluster per artist, weighted by decayed play/like mass, sorted
/// heaviest first. The sqrt makes the weight sublinear so a one-day binge on
/// a single artist can't own every daily-mix draw.
fn cluster_seeds(
    history: &[HistoryEntry],
    liked: &[Track],
    now: DateTime<Utc>,
) -> Vec<(f64, RecommendationSeed)> {
    let mut clusters: HashMap<String, (f64, RecommendationSeed)> = HashMap::new();
    for ws in weighted_seed_pool(history, liked, now) {
        let key = normalise_key(&ws.seed.artist);
        if key.is_empty() {
            continue;
        }
        let entry = clusters.entry(key).or_insert((0.0, ws.seed.clone()));
        entry.0 += ws.weight;
        // Prefer a SoundCloud-native track as the cluster's seed identity:
        // it hits the exact related-tracks endpoint instead of a text search.
        if !entry.1.track_uri.as_ref().is_some_and(is_soundcloud_track)
            && ws.seed.track_uri.as_ref().is_some_and(is_soundcloud_track)
        {
            entry.1 = ws.seed;
        }
    }
    let mut ranked: Vec<(f64, RecommendationSeed)> = clusters
        .into_values()
        .map(|(weight, seed)| (weight.sqrt(), seed))
        .collect();
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.artist.cmp(&b.1.artist))
    });
    ranked
}

/// Cap the number of seeds drawn from any single artist while preserving
/// the input order. Used to keep "Made for you" from looking like a
/// single-artist radio when the seed pool is skewed.
fn diverse_seeds(seeds: Vec<RecommendationSeed>, max_per_artist: usize) -> Vec<RecommendationSeed> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let key = normalise_key(&seed.artist);
        if key.is_empty() {
            out.push(seed);
            continue;
        }
        let count = counts.entry(key).or_insert(0);
        if *count < max_per_artist {
            *count += 1;
            out.push(seed);
        }
    }
    out
}

fn seed_from_track(track: &Track) -> Option<RecommendationSeed> {
    let artist = track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    seed_from_parts(&artist, &track.title, Some(track.uri.clone()))
}

fn seed_from_parts(
    artist: &str,
    title: &str,
    track_uri: Option<TrackUri>,
) -> Option<RecommendationSeed> {
    let artist = artist.trim();
    let title = title.trim();
    if artist.is_empty() || title.is_empty() {
        return None;
    }
    Some(RecommendationSeed {
        artist: artist.to_string(),
        title: title.to_string(),
        label: format!("{artist} — {title}"),
        track_uri,
    })
}

fn soundcloud_artist_uris(tracks: &[Track]) -> Vec<ArtistUri> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for track in tracks {
        for artist in &track.artists {
            if artist.uri.0.starts_with("soundcloud:user:") && seen.insert(artist.uri.0.clone()) {
                out.push(artist.uri.clone());
            }
        }
    }
    out
}

fn dedupe_tracks(tracks: Vec<Track>) -> Vec<Track> {
    let mut seen_uri = HashSet::<String>::new();
    let mut seen_key = HashSet::<String>::new();
    let mut out = Vec::new();
    for track in tracks {
        let key = track_key(&track);
        if seen_uri.insert(track.uri.0.clone()) && seen_key.insert(key) {
            out.push(track);
        }
    }
    out
}

fn dedupe_across_mixes(mixes: &mut [RecommendationMix]) {
    let mut seen = HashSet::<String>::new();
    let mut seen_canon = HashSet::<String>::new();
    for mix in mixes {
        mix.tracks.retain(|track| {
            let canon = discovery::canonical_title(&track.title);
            seen.insert(track.uri.0.clone())
                && seen.insert(track_key(track))
                && (canon.is_empty() || seen_canon.insert(canon))
        });
    }
}

fn track_key(track: &Track) -> String {
    let artist = track
        .artists
        .first()
        .map(|a| a.name.as_str())
        .unwrap_or_default();
    format!("{}|{}", normalise_key(artist), normalise_key(&track.title))
}

fn is_soundcloud_track(uri: &TrackUri) -> bool {
    uri.0.starts_with("soundcloud:track:")
}

fn normalise_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_api::{ArtistRef, ArtistUri, ProviderId, TrackUri};
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use std::time::Duration;

    fn plans_for(history: &[HistoryEntry], liked: &[Track]) -> Vec<ShelfPlan> {
        let mut rng = StdRng::seed_from_u64(1);
        let mut used = HashSet::new();
        build_shelf_plans(history, liked, &HashSet::new(), &mut used, &mut rng)
    }

    fn track(uri: &str, artist: &str, title: &str) -> Track {
        Track {
            uri: TrackUri(uri.into()),
            provider: if uri.starts_with("soundcloud:") {
                ProviderId::SoundCloud
            } else {
                ProviderId::Spotify
            },
            title: title.into(),
            artists: vec![ArtistRef {
                uri: ArtistUri(if uri.starts_with("soundcloud:") {
                    "soundcloud:user:1".into()
                } else {
                    "spotify:artist:1".into()
                }),
                name: artist.into(),
            }],
            album: None,
            duration: Duration::from_secs(180),
            cover_url: None,
            mbid: None,
            added_at: None,
        }
    }

    #[test]
    fn dedupe_tracks_uses_uri_and_artist_title() {
        let tracks = dedupe_tracks(vec![
            track("soundcloud:track:1", "A", "Song"),
            track("soundcloud:track:2", "A", "Song"),
            track("soundcloud:track:3", "A", "Other"),
        ]);
        assert_eq!(tracks.len(), 2);
    }

    #[test]
    fn seed_from_parts_skips_empty_rows() {
        assert!(seed_from_parts("", "Song", None).is_none());
        assert!(seed_from_parts("Artist", "", None).is_none());
        assert!(seed_from_parts("Artist", "Song", None).is_some());
    }

    #[test]
    fn cluster_seeds_prioritise_soundcloud_exact_tracks() {
        let liked = vec![
            track("spotify:track:1", "Artist A", "One"),
            track("soundcloud:track:2", "Artist B", "Two"),
            track("soundcloud:track:3", "Artist B", "Three"),
        ];
        let seeds = cluster_seeds(&[], &liked, Utc::now());
        assert_eq!(seeds.first().unwrap().1.artist, "Artist B");
    }

    #[test]
    fn build_shelf_plans_includes_aegis_chart_rows() {
        let liked = vec![track("soundcloud:track:1", "Artist A", "One")];
        let plans = plans_for(&[], &liked);
        assert!(plans.iter().any(|p| p.id == SHELF_MADE_FOR_YOU));
        assert!(plans.iter().any(|p| p.id == SHELF_TRENDING));
        assert!(plans.iter().any(|p| p.id == "genre-electronic"));
    }

    fn history(artist: &str, title: &str, uri: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            title: title.into(),
            artist: artist.into(),
            provider: match uri {
                Some(u) if u.starts_with("soundcloud:") => "SoundCloud".into(),
                _ => "Spotify".into(),
            },
            track_uri: uri.map(str::to_string),
            cover_url: None,
            played_at: Utc::now(),
        }
    }

    #[test]
    fn diverse_seeds_caps_per_artist_preserving_order() {
        let seeds = vec![
            seed_from_parts("A", "1", None).unwrap(),
            seed_from_parts("A", "2", None).unwrap(),
            seed_from_parts("B", "1", None).unwrap(),
            seed_from_parts("A", "3", None).unwrap(),
            seed_from_parts("C", "1", None).unwrap(),
            seed_from_parts("B", "2", None).unwrap(),
        ];
        let out = diverse_seeds(seeds, 1);
        let titles: Vec<_> = out
            .iter()
            .map(|s| (s.artist.as_str(), s.title.as_str()))
            .collect();
        assert_eq!(titles, vec![("A", "1"), ("B", "1"), ("C", "1")]);
    }

    #[test]
    fn diverse_seeds_allows_two_per_artist() {
        let seeds = vec![
            seed_from_parts("A", "1", None).unwrap(),
            seed_from_parts("A", "2", None).unwrap(),
            seed_from_parts("A", "3", None).unwrap(),
            seed_from_parts("B", "1", None).unwrap(),
        ];
        let out = diverse_seeds(seeds, 2);
        let a_count = out.iter().filter(|s| s.artist == "A").count();
        assert_eq!(a_count, 2);
        assert!(out.iter().any(|s| s.artist == "B"));
    }

    #[test]
    fn weighted_pool_keeps_history_identity_and_sums_weight() {
        // Same artist+title as play and like — one pool entry, the recent
        // play's URI wins the identity and the like only adds weight.
        let liked = vec![track("spotify:track:old", "Artist", "Track")];
        let history_log = vec![history("Artist", "Track", Some("soundcloud:track:new"))];
        let pool = weighted_seed_pool(&history_log, &liked, Utc::now());
        assert_eq!(pool.len(), 1);
        assert_eq!(
            pool[0].seed.track_uri.as_ref().map(|u| u.0.as_str()),
            Some("soundcloud:track:new")
        );
        // Fresh SC play (1.0 × 1.5) + non-SC like (0.25).
        assert!(pool[0].weight > 1.5);
    }

    #[test]
    fn cluster_seeds_recent_history_beats_old_like() {
        // A non-SC like vs a non-SC recent play. With time decay the fresh
        // play carries far more weight than a like's flat base.
        let liked = vec![track("spotify:track:l", "Old Artist", "Liked")];
        let history_log = vec![history("Recent Artist", "Played", None)];
        let seeds = cluster_seeds(&history_log, &liked, Utc::now());
        assert_eq!(
            seeds.first().map(|s| s.1.artist.as_str()),
            Some("Recent Artist")
        );
    }

    #[test]
    fn made_for_you_explores_beyond_head_artists() {
        // A heavy-rotation artist plus a long tail of one-play artists: the
        // explore quota must pull tail artists into the seed list.
        let mut history_log: Vec<HistoryEntry> = (0..20)
            .map(|i| {
                history(
                    "Big Artist",
                    &format!("Hit {i}"),
                    Some("soundcloud:track:big"),
                )
            })
            .collect();
        for i in 0..10 {
            history_log.push(history(&format!("Tail {i}"), "Deep Cut", None));
        }
        let pool = weighted_seed_pool(&history_log, &[], Utc::now());
        let mut rng = StdRng::seed_from_u64(3);
        let seeds =
            sample_seeds_with_explore(&pool, MADE_FOR_YOU_SEEDS, MADE_FOR_YOU_EXPLORE, &mut rng);
        let tail_count = seeds.iter().filter(|s| s.artist.starts_with("Tail")).count();
        assert!(tail_count >= 1, "no tail artist among seeds: {seeds:?}");
        let big_count = seeds.iter().filter(|s| s.artist == "Big Artist").count();
        assert!(big_count <= 2, "artist cap violated: {big_count}");
    }

    #[test]
    fn curate_tracks_excludes_caps_and_backfills() {
        let tracks = vec![
            track("soundcloud:track:0", "A", "S0"),
            track("soundcloud:track:1", "A", "S1"),
            track("soundcloud:track:2", "A", "S2"),
            track("soundcloud:track:3", "B", "S3"),
            track("soundcloud:track:4", "B", "S4"),
            track("soundcloud:track:5", "C", "S5"),
        ];
        let one_excluded: HashSet<String> = [track_key(&tracks[0])].into();
        let kept = curate_tracks(tracks.clone(), &one_excluded, 2, 6);
        // S0 excluded, artist cap 2 satisfied by the rest → 5 tracks.
        assert_eq!(kept.len(), 5);
        assert!(!kept.iter().any(|t| t.title == "S0"));

        // Artist cap: six tracks by A alone, cap 2, leaves 2 — under half of
        // limit 6, so the starvation backfill refills up to the limit.
        let same_artist: Vec<Track> = (0..6)
            .map(|i| track(&format!("soundcloud:track:a{i}"), "A", &format!("T{i}")))
            .collect();
        let kept = curate_tracks(same_artist, &HashSet::new(), 2, 6);
        assert_eq!(kept.len(), 6);

        // Everything excluded → backfill instead of an empty row.
        let all: HashSet<String> = tracks.iter().map(track_key).collect();
        let kept = curate_tracks(tracks, &all, 2, 4);
        assert_eq!(kept.len(), 4);
    }

    #[test]
    fn curate_tracks_collapses_same_song_from_different_uploaders() {
        let tracks = vec![
            track("soundcloud:track:1", "Original", "Cool Song"),
            track("soundcloud:track:2", "Reupload Guy", "Cool Song (Official Audio)"),
            track("spotify:track:3", "Original", "Cool Song"),
            track("soundcloud:track:4", "Other", "Different Song"),
        ];
        let kept = curate_tracks(tracks, &HashSet::new(), 3, 10);
        assert_eq!(kept.len(), 2, "duplicate song copies survived: {kept:?}");
    }

    #[test]
    fn filter_seed_variants_drops_reuploads_and_variants() {
        let seed = seed_from_parts("Artist", "Drench", None).unwrap();
        let tracks = vec![
            track("soundcloud:track:1", "Someone Else", "Drench"),
            track("soundcloud:track:2", "Other", "Drench (sped up)"),
            track("soundcloud:track:3", "Other", "Fresh Song (nightcore)"),
            track("soundcloud:track:4", "Other", "New Song"),
        ];
        let kept = filter_seed_variants(tracks, &seed);
        let titles: Vec<_> = kept.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["New Song"]);
    }

    #[test]
    fn mix_plans_pick_distinct_artists() {
        let mut history_log = Vec::new();
        for artist in ["A", "B", "C", "D", "E", "F", "G", "H"] {
            for i in 0..3 {
                history_log.push(history(artist, &format!("{artist} song {i}"), None));
            }
        }
        let mut rng = StdRng::seed_from_u64(9);
        let plans = build_mix_plans(&history_log, &[], &HashSet::new(), &HashSet::new(), &mut rng);
        assert_eq!(plans.len(), DAILY_MIX_COUNT);
        let artists: HashSet<String> = plans
            .iter()
            .map(|p| normalise_key(&p.seed.artist))
            .collect();
        assert_eq!(artists.len(), DAILY_MIX_COUNT, "duplicate mix artists");
    }

    #[test]
    fn recommendations_cache_roundtrips_via_serde() {
        let cache = RecommendationsCache {
            saved_at: Utc::now(),
            shelves: vec![RecommendationShelf {
                id: "made-for-you".into(),
                eyebrow: "Personal".into(),
                title: "Made for you".into(),
                subtitle: "subtitle".into(),
                seed_label: "label".into(),
                tracks: vec![track("soundcloud:track:1", "Artist", "Song")],
                is_loading: true,
                error: Some("ignored on serialise".into()),
                rerollable: true,
            }],
            mixes: vec![RecommendationMix {
                id: "daily-mix-0".into(),
                title: "Daily Mix 1".into(),
                subtitle: "subtitle".into(),
                seed_label: "label".into(),
                tracks: Vec::new(),
                is_loading: true,
                error: None,
                accent_index: 0,
            }],
            tiles: Vec::new(),
        };
        let raw = serde_json::to_string(&cache).expect("serialise");
        let parsed: RecommendationsCache = serde_json::from_str(&raw).expect("deserialise");
        assert_eq!(parsed.shelves.len(), 1);
        // Transient fields must default back to clean values.
        assert!(!parsed.shelves[0].is_loading);
        assert!(parsed.shelves[0].error.is_none());
        assert_eq!(parsed.shelves[0].tracks.len(), 1);
        assert_eq!(parsed.mixes.len(), 1);
        assert!(!parsed.mixes[0].is_loading);
    }

    #[test]
    fn made_for_you_caps_same_artist_seeds() {
        // 12 likes from the same artist; without diversity the shelf seed
        // list would be all "Hot Artist". Cap is 2.
        let liked: Vec<Track> = (0..12)
            .map(|i| {
                track(
                    &format!("soundcloud:track:{i}"),
                    "Hot Artist",
                    &format!("Song {i}"),
                )
            })
            .collect();
        let plans = plans_for(&[], &liked);
        let made = plans.iter().find(|p| p.id == SHELF_MADE_FOR_YOU).unwrap();
        match &made.kind {
            ShelfKind::RelatedAggregate { seeds, .. } => {
                assert!(
                    seeds.len() <= 2,
                    "expected ≤2 same-artist seeds, got {}",
                    seeds.len()
                );
            }
            _ => panic!("expected RelatedAggregate"),
        }
    }
}
