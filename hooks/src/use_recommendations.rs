//! Home "For You" / Explore recommendations.
//!
//! Mirrors the parts of Aegis that worked well: a SoundCloud-native Explore
//! landing with made-for-you rows, Daily Mix cards, because-you-played radio
//! seeds, artist-upload rows, and SoundCloud chart shelves.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use dioxus::prelude::*;
use discovery::{DiscoveryEngine, SimilarToSeed};
use player::HistoryEntry;
use provider_api::{ArtistUri, Provider, Query, Track, TrackUri};
use provider_soundcloud::SoundCloudProvider;

use crate::UseLibrary;
use crate::use_likes::LikedTrack;

const SHELF_LIMIT: usize = 14;
const MADE_FOR_YOU_SEEDS: usize = 10;
const DAILY_MIX_COUNT: usize = 4;
const DAILY_MIX_LIMIT: usize = 18;

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

#[derive(Clone, PartialEq)]
pub struct RecommendationShelf {
    pub id: String,
    pub eyebrow: String,
    pub title: String,
    pub subtitle: String,
    pub seed_label: String,
    pub tracks: Vec<Track>,
    pub is_loading: bool,
    pub error: Option<String>,
    pub rerollable: bool,
}

#[derive(Clone, PartialEq)]
pub struct RecommendationMix {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub seed_label: String,
    pub tracks: Vec<Track>,
    pub is_loading: bool,
    pub error: Option<String>,
    pub accent_index: usize,
}

#[derive(Clone, PartialEq)]
pub struct RecommendationTile {
    pub id: String,
    pub label: String,
    pub sub: String,
    pub glyph: String,
    pub cover_url: Option<String>,
    pub tracks: Vec<Track>,
    pub accent_index: usize,
}

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
        let shelf_plans = build_shelf_plans(&history, &liked_tracks, &offsets);
        let mix_plans = build_mix_plans(&history, &liked_tracks, &offsets);

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
            let next = generation.peek().wrapping_add(1);
            generation.set(next);
            next
        };

        let mut shelves_sig = self.shelves;
        let mut mixes_sig = self.mixes;
        let mut tiles_sig = self.tiles;
        let mut loading_sig = self.is_loading;
        let mut error_sig = self.error;
        let generation_sig = self.generation;
        let engine = self.engine.clone();
        let sc = self.sc.clone();
        let only_id = only.clone();

        mark_shelves_loading(&mut shelves_sig, &selected_shelves, only_id.as_deref());
        mark_mixes_loading(&mut mixes_sig, &selected_mixes, only_id.as_deref());

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
            tiles_sig.set(build_tiles(&shelves_now, &mixes_now));
            error_sig.set(if has_loaded_tracks { None } else { had_error });
            loading_sig.set(false);
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
    let shelves = use_signal(Vec::<RecommendationShelf>::new);
    let mixes = use_signal(Vec::<RecommendationMix>::new);
    let tiles = use_signal(Vec::<RecommendationTile>::new);
    let is_loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let offsets = use_signal(HashMap::<String, usize>::new);
    let generation = use_signal(|| 0u64);

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
            let has_content = !handle.shelves.peek().is_empty() || !handle.mixes.peek().is_empty();
            let in_flight = *handle.is_loading.peek();
            if history_len + spotify_len + local_len == 0 || has_content || in_flight {
                return;
            }
            handle.refresh_all();
        });
    }

    handle
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
        ShelfKind::Related { seed } => related_for_seed(sc, engine, &seed, SHELF_LIMIT).await,
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
    let result = related_for_seed(sc, engine, &plan.seed, DAILY_MIX_LIMIT)
        .await
        .map(|tracks| {
            tracks
                .into_iter()
                .filter(|t| !plan.exclude_keys.contains(&track_key(t)))
                .take(DAILY_MIX_LIMIT)
                .collect::<Vec<_>>()
        });

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

async fn related_aggregate(
    sc: Arc<SoundCloudProvider>,
    engine: Arc<DiscoveryEngine>,
    seeds: &[RecommendationSeed],
    exclude_keys: &HashSet<String>,
    limit: usize,
) -> Result<Vec<Track>, String> {
    let mut out = Vec::new();
    for seed in seeds.iter().take(MADE_FOR_YOU_SEEDS) {
        let tracks = related_for_seed(sc.clone(), engine.clone(), seed, 8).await?;
        for track in tracks {
            if !exclude_keys.contains(&track_key(&track)) {
                out.push(track);
            }
        }
        if dedupe_tracks(out.clone()).len() >= limit {
            break;
        }
    }
    Ok(dedupe_tracks(out).into_iter().take(limit).collect())
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
        return sc
            .related_tracks(uri, limit as u32)
            .await
            .map(dedupe_tracks)
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
    only_id: Option<&str>,
) {
    let mut shelves = shelves_sig.peek().clone();
    if shelves.is_empty() || only_id.is_none() {
        shelves = plans.iter().map(skeleton_shelf).collect();
    } else {
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
    only_id: Option<&str>,
) {
    let mut mixes = mixes_sig.peek().clone();
    if mixes.is_empty() || only_id.is_none() {
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
    if only_id.is_none() {
        shelves_sig.set(loaded);
        return;
    }
    let mut shelves = shelves_sig.peek().clone();
    for shelf in loaded {
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
    if only_id.is_none() {
        mixes_sig.set(loaded);
        return;
    }
    let mut mixes = mixes_sig.peek().clone();
    for mix in loaded {
        if let Some(existing) = mixes.iter_mut().find(|m| m.id == mix.id) {
            *existing = mix;
        } else {
            mixes.push(mix);
        }
    }
    mixes_sig.set(mixes);
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
    offsets: &HashMap<String, usize>,
) -> Vec<ShelfPlan> {
    let mut out = Vec::new();
    let liked_keys = liked.iter().map(track_key).collect::<HashSet<_>>();
    let all_seeds = seed_pool(history, liked);
    let recent_seeds = history_seeds(history);

    if !all_seeds.is_empty() {
        out.push(ShelfPlan {
            id: SHELF_MADE_FOR_YOU.to_string(),
            eyebrow: "Personal".into(),
            title: "Made for you".into(),
            subtitle: "Aegis-style SoundCloud related picks from your recent plays and likes."
                .into(),
            seed_label: "recent plays + likes".into(),
            rerollable: true,
            kind: ShelfKind::RelatedAggregate {
                seeds: rotate_seeds(&all_seeds, offset_for(offsets, SHELF_MADE_FOR_YOU))
                    .into_iter()
                    .take(MADE_FOR_YOU_SEEDS)
                    .collect(),
                exclude_keys: liked_keys.clone(),
            },
        });
    }

    if let Some(seed) = pick_seed(&recent_seeds, offset_for(offsets, SHELF_BECAUSE)) {
        out.push(ShelfPlan {
            id: SHELF_BECAUSE.to_string(),
            eyebrow: "Because you played".into(),
            title: format!("\"{}\"", seed.title),
            subtitle: format!("by {}", seed.artist),
            seed_label: seed.label.clone(),
            rerollable: true,
            kind: ShelfKind::Related { seed },
        });
    }

    let artist_uris = soundcloud_artist_uris(liked);
    if !artist_uris.is_empty() {
        out.push(ShelfPlan {
            id: SHELF_NEW_FROM_ARTISTS.to_string(),
            eyebrow: "New releases".into(),
            title: "From your artists".into(),
            subtitle: "Recent uploads from SoundCloud artists you saved.".into(),
            seed_label: format!("{} SoundCloud artists", artist_uris.len()),
            rerollable: true,
            kind: ShelfKind::ArtistUploads {
                artists: rotate_artist_uris(
                    &artist_uris,
                    offset_for(offsets, SHELF_NEW_FROM_ARTISTS),
                ),
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
                tracks: rotating_tracks(liked, SHELF_LIMIT, offset_for(offsets, SHELF_FROM_LIKES)),
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
    offsets: &HashMap<String, usize>,
) -> Vec<MixPlan> {
    let clusters = cluster_seeds(history, liked);
    if clusters.is_empty() {
        return Vec::new();
    }
    let liked_keys = liked.iter().map(track_key).collect::<HashSet<_>>();
    let rotated = rotate_seeds(&clusters, offset_for(offsets, MIXES_GROUP));
    rotated
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
            exclude_keys: liked_keys.clone(),
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

fn seed_pool(history: &[HistoryEntry], liked: &[Track]) -> Vec<RecommendationSeed> {
    let mut seeds = liked_seeds(liked);
    seeds.extend(history_seeds(history));
    dedupe_seeds(seeds)
}

fn history_seeds(entries: &[HistoryEntry]) -> Vec<RecommendationSeed> {
    entries
        .iter()
        .filter_map(|e| seed_from_parts(&e.artist, &e.title, e.track_uri.clone().map(TrackUri)))
        .collect()
}

fn liked_seeds(tracks: &[Track]) -> Vec<RecommendationSeed> {
    tracks.iter().filter_map(seed_from_track).collect()
}

fn cluster_seeds(history: &[HistoryEntry], liked: &[Track]) -> Vec<RecommendationSeed> {
    let mut clusters: HashMap<String, (usize, RecommendationSeed)> = HashMap::new();

    for seed in liked_seeds(liked).into_iter().chain(history_seeds(history)) {
        let key = normalise_key(&seed.artist);
        if key.is_empty() {
            continue;
        }
        let entry = clusters.entry(key).or_insert((0, seed.clone()));
        entry.0 += if seed.track_uri.as_ref().is_some_and(is_soundcloud_track) {
            3
        } else {
            1
        };
    }

    let mut ranked: Vec<(usize, RecommendationSeed)> = clusters.into_values().collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.artist.cmp(&b.1.artist)));
    ranked.into_iter().map(|(_, seed)| seed).collect()
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

fn offset_for(offsets: &HashMap<String, usize>, id: &str) -> usize {
    *offsets.get(id).unwrap_or(&0)
}

fn pick_seed(seeds: &[RecommendationSeed], offset: usize) -> Option<RecommendationSeed> {
    if seeds.is_empty() {
        return None;
    }
    seeds.get(offset % seeds.len()).cloned()
}

fn rotate_seeds(seeds: &[RecommendationSeed], offset: usize) -> Vec<RecommendationSeed> {
    if seeds.is_empty() {
        return Vec::new();
    }
    (0..seeds.len())
        .map(|i| seeds[(i + offset) % seeds.len()].clone())
        .collect()
}

fn rotate_artist_uris(artists: &[ArtistUri], offset: usize) -> Vec<ArtistUri> {
    if artists.is_empty() {
        return Vec::new();
    }
    (0..artists.len())
        .map(|i| artists[(i + offset) % artists.len()].clone())
        .collect()
}

fn rotating_tracks(tracks: &[Track], limit: usize, offset: usize) -> Vec<Track> {
    if tracks.is_empty() {
        return Vec::new();
    }
    let hour = (Utc::now().timestamp().max(0) as usize) / 3600;
    let start = (hour + offset * limit) % tracks.len();
    (0..tracks.len().min(limit))
        .map(|i| tracks[(start + i) % tracks.len()].clone())
        .collect()
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

fn dedupe_seeds(seeds: Vec<RecommendationSeed>) -> Vec<RecommendationSeed> {
    let mut seen = HashSet::<String>::new();
    let mut out = Vec::new();
    for seed in seeds {
        let key = format!(
            "{}|{}",
            normalise_key(&seed.artist),
            normalise_key(&seed.title)
        );
        if seen.insert(key) {
            out.push(seed);
        }
    }
    out
}

fn dedupe_across_mixes(mixes: &mut [RecommendationMix]) {
    let mut seen = HashSet::<String>::new();
    for mix in mixes {
        mix.tracks
            .retain(|track| seen.insert(track.uri.0.clone()) && seen.insert(track_key(track)));
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
    use std::time::Duration;

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
        let seeds = cluster_seeds(&[], &liked);
        assert_eq!(seeds.first().unwrap().artist, "Artist B");
    }

    #[test]
    fn build_shelf_plans_includes_aegis_chart_rows() {
        let liked = vec![track("soundcloud:track:1", "Artist A", "One")];
        let plans = build_shelf_plans(&[], &liked, &HashMap::new());
        assert!(plans.iter().any(|p| p.id == SHELF_MADE_FOR_YOU));
        assert!(plans.iter().any(|p| p.id == SHELF_TRENDING));
        assert!(plans.iter().any(|p| p.id == "genre-electronic"));
    }
}
