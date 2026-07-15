//! Home — one continuous editorial flow.
//!
//! A single expressive "Made for you" stage leads, followed by a calm stream
//! of uniform rails (recently played, daily mixes, related picks, scenes) that
//! all speak one card language. The ListenBrainz scrobble timeline closes the
//! page as the one dense list.
//!
//! Each rail degrades to its own empty / error state independently — Home
//! works on a fresh install (everything empty with CTAs) and fills in as
//! providers connect.
//!
//! Module layout: `stage` (the lead moment), `rails` (the shared rail shell +
//! shelf/mix rows), `activity` (recently played/liked), `feed` (ListenBrainz
//! timeline). Shared cards/helpers live here in the module root.

mod activity;
mod feed;
mod rails;
mod stage;

use dioxus::prelude::*;
use hooks::{
    RecommendationMix, RecommendationShelf, Track, use_history, use_library,
    use_listenbrainz_feed, use_recommendations,
};

use crate::parts::{PlayableButton, provider_badge_class};

use activity::{RecentlyLikedRail, RecentlyPlayedRail};
use feed::ListenedLately;
use rails::{DailyMixesRail, ShelfRail};
use stage::HomeStage;

#[component]
pub fn Home() -> Element {
    let history = use_history();
    let library = use_library();
    let feed = use_listenbrainz_feed();
    let recommendations = use_recommendations(library.clone(), history.deep_entries);

    let shelves = recommendations.shelves.read().clone();
    let mixes = recommendations.mixes.read().clone();
    let is_loading = *recommendations.is_loading.read();
    let error = recommendations.error.read().clone();
    let pool = merged_recommendation_tracks(&shelves, &mixes);

    let spotlight = shelf_by_id(&shelves, "made-for-you");
    let because = shelf_by_id(&shelves, "because-recent");
    let new_artists = shelf_by_id(&shelves, "new-from-artists");
    let from_likes = shelf_by_id(&shelves, "from-likes");
    let trending = shelf_by_id(&shelves, "trending-now");
    let scenes: Vec<RecommendationShelf> = shelves
        .iter()
        .filter(|s| s.id.starts_with("genre-"))
        .cloned()
        .collect();
    let rec_rails: Vec<RecommendationShelf> = [because, new_artists, from_likes, trending]
        .into_iter()
        .flatten()
        .collect();

    let history_entries = history.entries.read().clone();
    let recs_empty = shelves.is_empty() && mixes.is_empty() && !is_loading;

    rsx! {
        section { class: "page home-page",
            if let Some(msg) = error.as_ref() {
                div { class: "home-error", "{msg}" }
            }

            if recs_empty {
                EmptyState {
                    icon: "fa-solid fa-wand-magic-sparkles",
                    title: "No Home data yet.",
                    body: "Play or like a few tracks — nira builds your mixes, picks and scenes here.",
                }
            } else {
                HomeStage {
                    shelf: spotlight,
                    pool: pool.clone(),
                    recommendations: recommendations.clone(),
                    is_loading,
                }
            }

            RecentlyPlayedRail { entries: history_entries }
            RecentlyLikedRail { library: library.clone() }

            if !mixes.is_empty() {
                DailyMixesRail { mixes: mixes.clone(), recommendations: recommendations.clone() }
            }
            for shelf in rec_rails.iter() {
                ShelfRail {
                    key: "{shelf.id}",
                    shelf: shelf.clone(),
                    recommendations: recommendations.clone(),
                }
            }
            for shelf in scenes.iter() {
                ShelfRail {
                    key: "{shelf.id}",
                    shelf: shelf.clone(),
                    recommendations: recommendations.clone(),
                }
            }

            ListenedLately { feed }
        }
    }
}

// ─── Shared bits used across the submodules ────────────────────────────────

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
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
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
