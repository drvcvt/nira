//! Home — activity-first dashboard.
//!
//! Three equal-weight sections stacked top-to-bottom: Recently played (local
//! play-log), Recently liked (Spotify Liked Songs sorted by `added_at`), and
//! Listened lately (ListenBrainz `/user/<name>/listens`).
//!
//! Each section degrades to its own empty / error state independently — Home
//! works on a fresh install (everything empty with CTAs) and fills in as
//! providers connect.

use std::sync::Arc;

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    DiscoveryResult, HistoryEntry, Listen, Provider, Query, RecommendationMix, RecommendationShelf,
    RecommendationTile, Track, TrackUri, UseFeatured, UseLibrary, UseListenBrainzFeed,
    UseRecommendations, use_ctx_menu, use_featured, use_history, use_library,
    use_listenbrainz_feed, use_queue, use_recommendations, use_soundcloud, use_spotify,
};

use crate::parts::{PlayableButton, provider_badge_class};

#[component]
pub fn Home() -> Element {
    let history = use_history();
    let library = use_library();
    let feed = use_listenbrainz_feed();
    let featured = use_featured(library.clone());
    let recommendations = use_recommendations(library.clone(), history.entries);

    rsx! {
        section { class: "page home-page",
            h1 { "Home" }
            p { class: "hint",
                "What you've been doing lately — across providers and devices."
            }

            Featured { featured: featured.clone() }
            ForYouRecommendations { recommendations: recommendations.clone() }
            RecentlyPlayed { entries: history.entries.read().clone() }
            RecentlyLiked { library: library.clone() }
            ListenedLately { feed: feed.clone() }
        }
    }
}

// ─── Featured ("Try this") hero card ──────────────────────────────────────

#[component]
fn Featured(featured: UseFeatured) -> Element {
    let suggestion = featured.suggestion.read().clone();
    let seed = featured.seed.read().clone();
    let is_loading = *featured.is_loading.read();
    let error = featured.error.read().clone();
    let needs_library = *featured.needs_library.read();
    let queue = use_queue();

    rsx! {
        section { class: "home-section featured-section",
            header { class: "home-section-header",
                h2 { "Try this" }
                Button {
                    label: "Surprise me".to_string(),
                    icon: Some("fa-solid fa-shuffle".to_string()),
                    variant: ButtonVariant::Ghost,
                    disabled: needs_library || is_loading,
                    on_click: {
                        let featured = featured.clone();
                        move |_| featured.reroll()
                    },
                }
            }

            if needs_library {
                div { class: "home-empty",
                    div { class: "home-empty-glyph",
                        i { class: "fa-solid fa-wand-magic-sparkles" }
                    }
                    p { class: "home-empty-title", "Like a few songs first." }
                    p { class: "home-empty-body",
                        "Connect Spotify in Settings and like some tracks — nira picks "
                        "a random one and surfaces a similar track from across providers."
                    }
                }
            } else if let Some(rec) = suggestion.as_ref() {
                FeaturedCard {
                    result: rec.clone(),
                    seed: seed.clone(),
                    on_play: {
                        let queue = queue.clone();
                        let rec = rec.clone();
                        move |_| {
                            if let Some(track) = rec.play_target() {
                                queue.play_list(vec![track], 0);
                            }
                        }
                    },
                }
            } else if let Some(msg) = error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else {
                // Catch-all so the section always has visible content. This
                // covers the brief initial mount before `use_effect` fires
                // *and* in-flight reroll where is_loading=true but suggestion
                // hasn't landed yet.
                FeaturedSkeleton { seed: seed.clone() }
            }
        }
    }
}

#[component]
fn FeaturedCard(
    result: DiscoveryResult,
    seed: Option<Track>,
    on_play: EventHandler<()>,
) -> Element {
    let cover = result.cover_url.clone().unwrap_or_default();
    let title = result.title.clone();
    let artist = result.artist.clone();
    let has_spotify = result.spotify.is_some();
    let has_soundcloud = result.soundcloud.is_some();
    let rationale = result.rationale.clone();
    let seed_label = seed.as_ref().map(|t| {
        let artist = t
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} — {}", artist, t.title)
    });
    let ctx = use_ctx_menu();
    let ctx_target = result.play_target();

    rsx! {
        article {
            class: "featured-card",
            oncontextmenu: {
                let ctx_target = ctx_target.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let Some(t) = ctx_target.clone() else { return; };
                    let pos = e.data.client_coordinates();
                    ctx.open(pos.x, pos.y, t);
                }
            },
            div { class: "featured-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    div { class: "cover-card-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
            }
            div { class: "featured-meta",
                span { class: "featured-eyebrow", "Recommended for you" }
                h3 { class: "featured-title", "{title}" }
                p { class: "featured-artist", "{artist}" }
                if let Some(label) = seed_label.as_ref() {
                    p { class: "featured-rationale",
                        title: "{rationale}",
                        i { class: "fa-solid fa-link" }
                        " based on "
                        span { class: "featured-seed", "{label}" }
                    }
                }
                div { class: "featured-actions",
                    Button {
                        label: "Play".to_string(),
                        icon: Some("fa-solid fa-play".to_string()),
                        variant: ButtonVariant::Primary,
                        on_click: move |_| on_play.call(()),
                    }
                    div { class: "featured-badges",
                        if has_spotify {
                            span { class: "track-badge spotify", "S" }
                        }
                        if has_soundcloud {
                            span { class: "track-badge soundcloud", "SC" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FeaturedSkeleton(seed: Option<Track>) -> Element {
    let seed_label = seed.as_ref().map(|t| {
        format!(
            "{} — {}",
            t.artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            t.title
        )
    });
    rsx! {
        article { class: "featured-card featured-card-skeleton",
            div { class: "featured-art featured-art-skeleton" }
            div { class: "featured-meta",
                span { class: "featured-eyebrow", "Searching neighbours…" }
                div { class: "featured-skeleton-line wide" }
                div { class: "featured-skeleton-line" }
                if let Some(label) = seed_label.as_ref() {
                    p { class: "featured-rationale",
                        i { class: "fa-solid fa-link" }
                        " based on "
                        span { class: "featured-seed", "{label}" }
                    }
                }
            }
        }
    }
}

// ─── For You ─────────────────────────────────────────────────────────────
//
// Layout:
//
// 1. Slim header (eyebrow + title + actions). No card chrome — just the
//    page-level hierarchy.
// 2. Dashboard row: 2-col on desktop, 1-col below ~1080px content width.
//      - left: Spotlight ("Made for you") — lead art + secondary picks.
//      - right: 2×2 Daily Mixes grid + 1–2 quick tiles below.
// 3. Rails group: compact horizontal scroll rails with smaller cards and
//    no card chrome — Because You Played, From Your Artists, From Your
//    Likes, Trending. Hierarchy via spacing, not duplicated borders.
// 4. Scenes panel: 2-col grid of compact rails for SoundCloud genres.

#[component]
fn ForYouRecommendations(recommendations: UseRecommendations) -> Element {
    let shelves = recommendations.shelves.read().clone();
    let mixes = recommendations.mixes.read().clone();
    let tiles = recommendations.tiles.read().clone();
    let is_loading = *recommendations.is_loading.read();
    let error = recommendations.error.read().clone();
    let pool = merged_recommendation_tracks(&shelves, &mixes);
    let spotlight = shelf_by_id(&shelves, "made-for-you");
    let because = shelf_by_id(&shelves, "because-recent");
    let new_artists = shelf_by_id(&shelves, "new-from-artists");
    let from_likes = shelf_by_id(&shelves, "from-likes");
    let trending = shelf_by_id(&shelves, "trending-now");
    let scenes = shelves
        .iter()
        .filter(|s| s.id.starts_with("genre-"))
        .cloned()
        .collect::<Vec<_>>();
    let queue = use_queue();
    let dashboard_has_content = spotlight.is_some() || !mixes.is_empty() || !tiles.is_empty();
    let rails: Vec<RecommendationShelf> = [because, new_artists, from_likes, trending]
        .into_iter()
        .flatten()
        .collect();
    let everything_empty =
        tiles.is_empty() && shelves.is_empty() && mixes.is_empty() && !is_loading;

    rsx! {
        section { class: "home-section for-you-section",
            header { class: "for-you-header",
                div { class: "for-you-header-text",
                    span { class: "for-you-eyebrow", "Made for you" }
                    h2 { class: "for-you-title", "For You" }
                    p { class: "for-you-sub",
                        if is_loading && pool.is_empty() {
                            "Loading mixes, related tracks and scene rows…"
                        } else if pool.is_empty() {
                            "Play or like a few tracks — nira will build your dashboard here."
                        } else {
                            "{pool.len()} tracks across mixes, related picks and scene rows."
                        }
                    }
                }
                div { class: "for-you-header-actions",
                    Button {
                        label: "Refresh".to_string(),
                        icon: Some(if is_loading { "fa-solid fa-circle-notch fa-spin".to_string() } else { "fa-solid fa-rotate".to_string() }),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: is_loading,
                        on_click: {
                            let recommendations = recommendations.clone();
                            move |_| recommendations.refresh_all()
                        },
                    }
                    Button {
                        label: "Surprise me".to_string(),
                        icon: Some("fa-solid fa-dice".to_string()),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: pool.is_empty(),
                        on_click: {
                            let queue = queue.clone();
                            let pool = pool.clone();
                            move |_| {
                                if pool.is_empty() { return; }
                                let idx = (chrono::Utc::now().timestamp_millis().unsigned_abs() as usize) % pool.len();
                                queue.play_context(pool.clone(), idx);
                            }
                        },
                    }
                    Button {
                        label: "Shuffle".to_string(),
                        icon: Some("fa-solid fa-shuffle".to_string()),
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Sm,
                        disabled: pool.is_empty(),
                        on_click: {
                            let queue = queue.clone();
                            let pool = pool.clone();
                            move |_| queue.play_context(pool.clone(), 0)
                        },
                    }
                }
            }

            if let Some(msg) = error.as_ref() {
                div { class: "home-error", "{msg}" }
            }

            if everything_empty {
                EmptyState {
                    icon: "fa-solid fa-wand-magic-sparkles",
                    title: "No Explore data yet.",
                    body: "Play or like a few tracks — nira builds Aegis-style shelves here.",
                }
            } else {
                if dashboard_has_content {
                    div { class: "for-you-dashboard",
                        if let Some(shelf) = spotlight.clone() {
                            ForYouSpotlightCard {
                                shelf,
                                recommendations: recommendations.clone(),
                            }
                        }
                        div { class: "for-you-dashboard-side",
                            if !mixes.is_empty() {
                                DailyMixesGrid {
                                    mixes: mixes.clone(),
                                    recommendations: recommendations.clone(),
                                }
                            }
                            if !tiles.is_empty() {
                                ForYouQuickTiles { tiles: tiles.clone() }
                            }
                        }
                    }
                }

                if !rails.is_empty() {
                    section { class: "for-you-rails",
                        for shelf in rails.iter() {
                            CompactRail {
                                key: "{shelf.id}",
                                shelf: shelf.clone(),
                                recommendations: recommendations.clone(),
                            }
                        }
                    }
                }

                if !scenes.is_empty() {
                    section { class: "for-you-scenes",
                        header { class: "for-you-scenes-head",
                            div {
                                span { class: "shelf-eyebrow", "Scenes" }
                                h3 { "SoundCloud lanes" }
                                p { class: "for-you-subtitle",
                                    "Compact genre rows — not another wall of identical shelves."
                                }
                            }
                        }
                        div { class: "for-you-scenes-grid",
                            for shelf in scenes.iter() {
                                CompactRail {
                                    key: "{shelf.id}",
                                    shelf: shelf.clone(),
                                    recommendations: recommendations.clone(),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ForYouQuickTiles(tiles: Vec<RecommendationTile>) -> Element {
    rsx! {
        div { class: "quick-tiles",
            for tile in tiles.iter() {
                ForYouTile { key: "{tile.id}", tile: tile.clone() }
            }
        }
    }
}

#[component]
fn ForYouTile(tile: RecommendationTile) -> Element {
    let queue = use_queue();
    let tracks = tile.tracks.clone();
    let hue = accent_hue(tile.accent_index);
    let cover = tile.cover_url.clone().unwrap_or_default();

    rsx! {
        button {
            class: "quick-tile",
            style: "--tile-hue: {hue};",
            disabled: tracks.is_empty(),
            onclick: move |_| queue.play_context(tracks.clone(), 0),
            div { class: "quick-tile-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    span { class: "quick-tile-glyph", "{tile.glyph}" }
                }
            }
            div { class: "quick-tile-text",
                div { class: "quick-tile-label", "{tile.label}" }
                div { class: "quick-tile-sub", "{tile.sub}" }
            }
            span { class: "quick-tile-play", i { class: "fa-solid fa-play" } }
        }
    }
}

#[component]
fn DailyMixesGrid(mixes: Vec<RecommendationMix>, recommendations: UseRecommendations) -> Element {
    let any_loading = mixes.iter().any(|m| m.is_loading);
    rsx! {
        section { class: "daily-mixes",
            header { class: "daily-mixes-head",
                div { class: "daily-mixes-titles",
                    span { class: "shelf-eyebrow", "Clusters" }
                    h3 { class: "daily-mixes-title", "Daily Mixes" }
                }
                Button {
                    label: "Reroll".to_string(),
                    icon: Some("fa-solid fa-shuffle".to_string()),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    disabled: any_loading,
                    on_click: {
                        let recommendations = recommendations.clone();
                        move |_| recommendations.reroll_shelf("daily-mixes".to_string())
                    },
                }
            }
            div { class: "daily-mix-grid",
                for mix in mixes.iter() {
                    DailyMixCard { key: "{mix.id}", mix: mix.clone() }
                }
            }
        }
    }
}

#[component]
fn DailyMixCard(mix: RecommendationMix) -> Element {
    let queue = use_queue();
    let tracks = mix.tracks.clone();
    let artworks = mix_artworks(&tracks);
    let hue = accent_hue(mix.accent_index);

    rsx! {
        button {
            class: "mix-card",
            style: "--mix-hue: {hue};",
            disabled: tracks.is_empty() || mix.is_loading,
            onclick: move |_| queue.play_context(tracks.clone(), 0),
            div { class: "mix-card-art",
                if mix.is_loading && artworks.is_empty() {
                    div { class: "mix-card-cover mix-card-empty", i { class: "fa-solid fa-circle-notch fa-spin" } }
                } else if artworks.len() >= 4 {
                    div { class: "mix-mosaic",
                        for src in artworks.iter().take(4) {
                            div { key: "{src}", class: "mix-mosaic-cell", style: "background-image: url('{src}')" }
                        }
                    }
                } else if let Some(src) = artworks.first() {
                    img { class: "mix-card-cover", src: "{src}", alt: "", loading: "lazy" }
                } else {
                    div { class: "mix-card-cover mix-card-empty", span { "♫" } }
                }
                div { class: "mix-card-overlay",
                    div { class: "mix-card-label", "{mix.title}" }
                }
            }
            div { class: "mix-card-title", "{mix.title}" }
            div { class: "mix-card-sub", title: "{mix.subtitle}", "{mix.subtitle}" }
        }
    }
}

#[component]
fn ForYouSpotlightCard(shelf: RecommendationShelf, recommendations: UseRecommendations) -> Element {
    let tracks = shelf.tracks.clone();
    let shelf_id = shelf.id.clone();
    let queue = use_queue();

    rsx! {
        section { class: "for-you-spotlight",
            header { class: "for-you-spotlight-head",
                div { class: "for-you-spotlight-titles",
                    span { class: "shelf-eyebrow", "{shelf.eyebrow}" }
                    h3 { class: "for-you-spotlight-title", "{shelf.title}" }
                    if !shelf.seed_label.is_empty() {
                        p { class: "for-you-seed",
                            "based on "
                            span { "{shelf.seed_label}" }
                        }
                    }
                    p { class: "for-you-subtitle", "{shelf.subtitle}" }
                }
                div { class: "for-you-spotlight-actions",
                    Button {
                        label: "Play".to_string(),
                        icon: Some("fa-solid fa-play".to_string()),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: tracks.is_empty(),
                        on_click: {
                            let queue = queue.clone();
                            let tracks = tracks.clone();
                            move |_| queue.play_context(tracks.clone(), 0)
                        },
                    }
                    if shelf.rerollable {
                        Button {
                            label: if shelf.is_loading { "Rerolling".to_string() } else { "Reroll".to_string() },
                            icon: Some(if shelf.is_loading { "fa-solid fa-circle-notch fa-spin".to_string() } else { "fa-solid fa-shuffle".to_string() }),
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            disabled: shelf.is_loading,
                            on_click: {
                                let recommendations = recommendations.clone();
                                move |_| recommendations.reroll_shelf(shelf_id.clone())
                            },
                        }
                    }
                }
            }
            if let Some(msg) = shelf.error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else if shelf.is_loading && shelf.tracks.is_empty() {
                SpotlightSkeleton {}
            } else if shelf.tracks.is_empty() {
                div { class: "shelf-empty", "Nothing here yet." }
            } else {
                ForYouSpotlightBody { tracks: tracks.clone() }
            }
        }
    }
}

#[component]
fn ForYouSpotlightBody(tracks: Vec<Track>) -> Element {
    let first = tracks.first().cloned();
    rsx! {
        div { class: "for-you-spotlight-body",
            if let Some(track) = first {
                div { class: "for-you-spotlight-main",
                    TrackCard { track: track.clone(), tracks: tracks.clone(), index: 0 }
                }
            }
            div { class: "for-you-spotlight-side",
                for (idx, track) in tracks.iter().enumerate().skip(1).take(6) {
                    TrackCard {
                        key: "{track.uri.0}",
                        track: track.clone(),
                        tracks: tracks.clone(),
                        index: idx,
                    }
                }
            }
        }
    }
}

#[component]
fn SpotlightSkeleton() -> Element {
    rsx! {
        div { class: "for-you-spotlight-body",
            div { class: "for-you-spotlight-main",
                div { class: "cover-card for-you-skeleton-card",
                    div { class: "cover-card-art" }
                    div { class: "featured-skeleton-line wide" }
                    div { class: "featured-skeleton-line" }
                }
            }
            div { class: "for-you-spotlight-side",
                for idx in 0..4 {
                    div { key: "{idx}", class: "cover-card for-you-skeleton-card",
                        div { class: "cover-card-art" }
                        div { class: "featured-skeleton-line wide" }
                    }
                }
            }
        }
    }
}

#[component]
fn CompactRail(shelf: RecommendationShelf, recommendations: UseRecommendations) -> Element {
    let tracks = shelf.tracks.clone();
    let shelf_id = shelf.id.clone();
    let queue = use_queue();

    rsx! {
        section { class: "for-you-rail",
            header { class: "for-you-rail-head",
                div { class: "for-you-rail-titles",
                    span { class: "shelf-eyebrow", "{shelf.eyebrow}" }
                    h4 { class: "for-you-rail-title", "{shelf.title}" }
                    if !shelf.subtitle.is_empty() {
                        p { class: "for-you-rail-sub", "{shelf.subtitle}" }
                    }
                }
                div { class: "for-you-rail-actions",
                    Button {
                        label: "Play".to_string(),
                        icon: Some("fa-solid fa-play".to_string()),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: tracks.is_empty(),
                        on_click: {
                            let queue = queue.clone();
                            let tracks = tracks.clone();
                            move |_| queue.play_context(tracks.clone(), 0)
                        },
                    }
                    if shelf.rerollable {
                        Button {
                            label: if shelf.is_loading { "Rerolling".to_string() } else { "Reroll".to_string() },
                            icon: Some(if shelf.is_loading { "fa-solid fa-circle-notch fa-spin".to_string() } else { "fa-solid fa-shuffle".to_string() }),
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            disabled: shelf.is_loading,
                            on_click: {
                                let recommendations = recommendations.clone();
                                move |_| recommendations.reroll_shelf(shelf_id.clone())
                            },
                        }
                    }
                }
            }
            if let Some(msg) = shelf.error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else if shelf.is_loading && shelf.tracks.is_empty() {
                div { class: "for-you-rail-row for-you-loading-row",
                    for idx in 0..6 {
                        div { key: "{idx}", class: "cover-card for-you-skeleton-card",
                            div { class: "cover-card-art" }
                            div { class: "featured-skeleton-line wide" }
                            div { class: "featured-skeleton-line" }
                        }
                    }
                }
            } else if shelf.tracks.is_empty() {
                div { class: "shelf-empty", "Nothing here yet." }
            } else {
                div { class: "for-you-rail-row",
                    for (idx, track) in tracks.iter().enumerate() {
                        TrackCard {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            tracks: tracks.clone(),
                            index: idx,
                        }
                    }
                }
            }
        }
    }
}

fn shelf_by_id(shelves: &[RecommendationShelf], id: &str) -> Option<RecommendationShelf> {
    shelves.iter().find(|s| s.id == id).cloned()
}

fn merged_recommendation_tracks(
    shelves: &[RecommendationShelf],
    mixes: &[RecommendationMix],
) -> Vec<Track> {
    let mut seen = std::collections::HashSet::<String>::new();
    let mut out = Vec::new();
    for track in shelves
        .iter()
        .flat_map(|s| s.tracks.iter())
        .chain(mixes.iter().flat_map(|m| m.tracks.iter()))
    {
        if seen.insert(track.uri.0.clone()) {
            out.push(track.clone());
        }
    }
    out
}

fn mix_artworks(tracks: &[Track]) -> Vec<String> {
    let mut seen = std::collections::HashSet::<String>::new();
    let mut out = Vec::new();
    for track in tracks {
        let Some(url) = track.cover_url.as_ref() else {
            continue;
        };
        if seen.insert(url.clone()) {
            out.push(url.clone());
        }
        if out.len() >= 4 {
            break;
        }
    }
    out
}

fn accent_hue(idx: usize) -> u16 {
    const HUES: &[u16] = &[200, 20, 280, 140, 340, 60, 180, 310];
    HUES[idx % HUES.len()]
}

// ─── Recently played ──────────────────────────────────────────────────────

#[component]
fn RecentlyPlayed(entries: Vec<HistoryEntry>) -> Element {
    rsx! {
        section { class: "home-section",
            header { class: "home-section-header",
                h2 { "Recently played" }
                span { class: "home-section-meta",
                    if entries.is_empty() {
                        "nothing yet"
                    } else {
                        "{entries.len()} most recent"
                    }
                }
            }

            if entries.is_empty() {
                EmptyState {
                    icon: "fa-solid fa-clock-rotate-left",
                    title: "No plays yet.",
                    body: "Hit play in the transport bar (or pick a track from Search / Library) — it shows up here next time you open Home.",
                }
            } else {
                div { class: "cover-row",
                    for (idx, entry) in entries.iter().enumerate() {
                        HistoryCard {
                            entry: entry.clone(),
                            entries: entries.clone(),
                            index: idx,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn HistoryCard(entry: HistoryEntry, entries: Vec<HistoryEntry>, index: usize) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let sc: Arc<dyn Provider> = use_soundcloud();
    let sp: Arc<dyn Provider> = use_spotify();
    let cover = entry.cover_url.clone().unwrap_or_default();
    let badge_class = badge_class_for(&entry.provider);
    let badge = badge_glyph_for(&entry.provider);
    let title = entry.title.clone();
    let artist = entry.artist.clone();
    let played_label = format_relative(entry.played_at);
    let sc_for_click = sc.clone();
    let sp_for_click = sp.clone();
    let sc_for_context = sc.clone();
    let sp_for_context = sp.clone();

    rsx! {
        button {
            class: "cover-card clickable",
            title: "{title} — {artist}\nplayed {played_label}",
            onclick: move |_| {
                let queue = queue.clone();
                let sc = sc_for_click.clone();
                let sp = sp_for_click.clone();
                let entries = entries.clone();
                spawn(async move {
                    let mut tracks = Vec::<Track>::new();
                    let mut start_idx = None::<usize>;
                    for (i, row) in entries.iter().enumerate() {
                        if let Some(track) = resolve_history_entry(sc.clone(), sp.clone(), row).await {
                            if i == index {
                                start_idx = Some(tracks.len());
                            }
                            tracks.push(track);
                        }
                    }
                    if let Some(start) = start_idx {
                        queue.play_context(tracks, start);
                    }
                });
            },
            oncontextmenu: {
                let entry = entry.clone();
                let sc = sc_for_context.clone();
                let sp = sp_for_context.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    let sc = sc.clone();
                    let sp = sp.clone();
                    let entry = entry.clone();
                    spawn(async move {
                        if let Some(track) = resolve_history_entry(sc, sp, &entry).await {
                            ctx.open(pos.x, pos.y, track);
                        }
                    });
                }
            },
            div { class: "cover-card-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    div { class: "cover-card-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
                span { class: "{badge_class}", "{badge}" }
            }
            div { class: "cover-card-title", "{title}" }
            div { class: "cover-card-sub", "{artist}" }
            div { class: "cover-card-time", "{played_label}" }
        }
    }
}

async fn resolve_history_entry(
    sc: Arc<dyn Provider>,
    sp: Arc<dyn Provider>,
    entry: &HistoryEntry,
) -> Option<Track> {
    let exact = match (entry.provider.as_str(), entry.track_uri.as_ref()) {
        ("Spotify", Some(uri)) => sp.track(&TrackUri(uri.clone())).await.ok(),
        ("SoundCloud", Some(uri)) => sc.track(&TrackUri(uri.clone())).await.ok(),
        _ => None,
    };
    if exact.is_some() {
        return exact;
    }

    let q = Query {
        text: format!("{} {}", entry.artist, entry.title),
        limit: Some(5),
    };
    match entry.provider.as_str() {
        "Spotify" => sp
            .search(&q)
            .await
            .ok()
            .and_then(|r| r.tracks.into_iter().next()),
        "SoundCloud" => sc
            .search(&q)
            .await
            .ok()
            .and_then(|r| r.tracks.into_iter().next()),
        _ => None,
    }
}

// ─── Recently liked ────────────────────────────────────────────────────────

#[component]
fn RecentlyLiked(library: UseLibrary) -> Element {
    let tracks = library.recently_liked.read().clone();
    let total_liked = library.liked.read().len();
    let is_loading = *library.is_loading.read();
    let error = library.error.read().clone();
    // Use the same sorted slice for both the visible cards *and* the
    // playback context. The earlier code called `full_list.read().clone()`
    // inside the loop, which copied the full liked vector per card —
    // O(visible × liked) and noticeable at ~900 likes. With ~8 visible
    // cards the playback context here is also short, so we can share
    // `tracks` directly.
    let context_tracks = tracks.clone();

    rsx! {
        section { class: "home-section",
            header { class: "home-section-header",
                h2 { "Recently liked" }
                span { class: "home-section-meta",
                    if total_liked > 0 {
                        "{total_liked} liked songs"
                    } else if is_loading {
                        "loading…"
                    } else {
                        ""
                    }
                }
            }

            if tracks.is_empty() && error.is_some() {
                div { class: "home-error", "{error.as_deref().unwrap_or_default()}" }
            } else if tracks.is_empty() && !is_loading {
                EmptyState {
                    icon: "fa-solid fa-heart",
                    title: "No liked songs yet.",
                    body: "Connect Spotify in Settings — your liked-songs library shows up here once the first sync finishes.",
                }
            } else {
                div { class: "cover-row",
                    for (idx, track) in tracks.iter().enumerate() {
                        TrackCard {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            tracks: context_tracks.clone(),
                            index: idx,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TrackCard(track: Track, tracks: Vec<Track>, index: usize) -> Element {
    let cover = track.cover_url.clone().unwrap_or_default();
    let title = track.title.clone();
    let artist = track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let badge_class = provider_badge_class(track.provider);

    rsx! {
        PlayableButton {
            track: track.clone(),
            tracks,
            index,
            class: "cover-card clickable".to_string(),
            title: format!("{title} — {artist}"),
            div { class: "cover-card-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    div { class: "cover-card-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
                span { class: "{badge_class}", "{track.provider.badge()}" }
            }
            div { class: "cover-card-title", "{title}" }
            div { class: "cover-card-sub", "{artist}" }
        }
    }
}

// ─── Listened lately (ListenBrainz feed) ──────────────────────────────────

#[component]
fn ListenedLately(feed: UseListenBrainzFeed) -> Element {
    let listens = feed.listens.read().clone();
    let is_loading = *feed.is_loading.read();
    let error = feed.error.read().clone();
    let needs_config = *feed.needs_config.read();

    rsx! {
        section { class: "home-section",
            header { class: "home-section-header",
                h2 { "Listened lately" }
                span { class: "home-section-meta",
                    if needs_config {
                        "ListenBrainz off"
                    } else if is_loading {
                        "fetching…"
                    } else if listens.is_empty() {
                        "no listens yet"
                    } else {
                        "{listens.len()} latest"
                    }
                }
            }

            if needs_config {
                EmptyState {
                    icon: "fa-solid fa-rss",
                    title: "Connect ListenBrainz.",
                    body: "Add your ListenBrainz username (and optionally a token for scrobbling) in Settings. Your listening trail across players surfaces here once configured.",
                }
            } else if let Some(msg) = error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else if listens.is_empty() && !is_loading {
                EmptyState {
                    icon: "fa-solid fa-rss",
                    title: "No listens reported yet.",
                    body: "Scrobble a few plays — they'll show up here on the next refresh.",
                }
            } else {
                ul { class: "feed-list",
                    for listen in listens.iter() {
                        FeedLine { listen: listen.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn FeedLine(listen: Listen) -> Element {
    let badge_class = badge_class_for(listen.source.as_deref().unwrap_or(""));
    let badge = badge_glyph_for(listen.source.as_deref().unwrap_or(""));
    let when = format_relative(listen.listened_at);
    let source_label = listen.source.clone().unwrap_or_else(|| "elsewhere".into());

    rsx! {
        li { class: "feed-line",
            span { class: "{badge_class} feed-line-dot", "{badge}" }
            div { class: "feed-line-meta",
                span { class: "feed-line-title", "{listen.title}" }
                span { class: "feed-line-artist", " — {listen.artist}" }
            }
            span { class: "feed-line-source", "{source_label}" }
            span { class: "feed-line-time", "{when}" }
        }
    }
}

// ─── Shared bits ───────────────────────────────────────────────────────────

#[component]
fn EmptyState(icon: &'static str, title: &'static str, body: &'static str) -> Element {
    rsx! {
        div { class: "home-empty",
            div { class: "home-empty-glyph",
                i { class: "{icon}" }
            }
            p { class: "home-empty-title", "{title}" }
            p { class: "home-empty-body", "{body}" }
        }
    }
}

/// Map a free-text provider/source string to a badge class. We accept the
/// generic `ProviderId::label()` strings ("Spotify"/"SoundCloud") as well as
/// the looser tags ListenBrainz emits in `listening_from` (e.g. "spotify",
/// "lastfm import"). Everything that doesn't match falls back to neutral.
fn badge_class_for(s: &str) -> &'static str {
    let lower = s.to_ascii_lowercase();
    if lower.contains("spotify") {
        "track-badge spotify"
    } else if lower.contains("soundcloud") {
        "track-badge soundcloud"
    } else {
        "track-badge"
    }
}

fn badge_glyph_for(s: &str) -> &'static str {
    let lower = s.to_ascii_lowercase();
    if lower.contains("spotify") {
        "S"
    } else if lower.contains("soundcloud") {
        "SC"
    } else {
        "·"
    }
}

/// Coarse human-readable relative time. Used in both row tooltips and feed
/// lines — anything more precise than minute-of-hour is noise for this
/// surface.
fn format_relative(when: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(when);
    let secs = delta.num_seconds().max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 60 * 60 {
        let m = secs / 60;
        format!("{m} min ago")
    } else if secs < 60 * 60 * 24 {
        let h = secs / 3600;
        format!("{h} h ago")
    } else if secs < 60 * 60 * 24 * 7 {
        let d = secs / 86_400;
        format!("{d} d ago")
    } else {
        when.format("%Y-%m-%d").to_string()
    }
}
