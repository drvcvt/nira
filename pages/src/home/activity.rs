//! Activity rails — recently played (local play-log) and recently liked
//! (Spotify Liked Songs).

use std::sync::Arc;

use dioxus::prelude::*;
use hooks::{
    ArtistRef, ArtistUri, HistoryEntry, Provider, ProviderId, Query, Track, TrackUri, UseLibrary,
    use_ctx_menu, use_local_library, use_queue, use_soundcloud, use_spotify,
};

use super::rails::{Rail, SkeletonRow};
use super::{EmptyState, TrackCard, badge_class_for, badge_glyph_for, format_relative};
use crate::parts::TrackCtx;

#[component]
pub(super) fn RecentlyPlayedRail(entries: Vec<HistoryEntry>) -> Element {
    rsx! {
        Rail { eyebrow: "Activity".to_string(), title: "Recently played".to_string(),
            if entries.is_empty() {
                EmptyState {
                    icon: "fa-solid fa-clock-rotate-left",
                    title: "No plays yet.",
                    body: "Hit play in the transport bar (or pick a track from Search / Library) — it shows up here next time you open Home.",
                }
            } else {
                div { class: "home-rail-row",
                    for (idx, entry) in entries.iter().enumerate() {
                        HistoryCard {
                            key: "{idx}",
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
    let local = use_local_library();
    let cover = entry.cover_url.clone().unwrap_or_default();
    let badge_class = badge_class_for(&entry.provider);
    let badge = badge_glyph_for(&entry.provider);
    let title = entry.title.clone();
    let artist = entry.artist.clone();
    let clicked_title = title.clone();
    let played_label = format_relative(entry.played_at);
    let providers_for_click = (sc.clone(), sp.clone());
    let providers_for_context = (sc, sp);

    rsx! {
        button {
            class: "cover-card clickable",
            title: "{title} — {artist}\nplayed {played_label}",
            onclick: move |_| {
                let queue = queue.clone();
                let (sc, sp) = providers_for_click.clone();
                let local_tracks = local.tracks.peek().clone();
                let entries = entries.clone();
                let clicked_title = clicked_title.clone();
                spawn(async move {
                    // Resolve the whole strip concurrently — serially this
                    // was up to ~2 network round trips per entry before any
                    // audio started.
                    let resolved = futures_util::future::join_all(entries.iter().map(|row| {
                        resolve_history_entry(sc.clone(), sp.clone(), &local_tracks, row)
                    }))
                    .await;
                    let mut tracks = Vec::<Track>::new();
                    let mut start_idx = None::<usize>;
                    for (i, track) in resolved.into_iter().enumerate() {
                        if let Some(track) = track {
                            if i == index {
                                start_idx = Some(tracks.len());
                            }
                            tracks.push(track);
                        }
                    }
                    match start_idx {
                        Some(start) => queue.play_context(tracks, start),
                        // The clicked entry didn't resolve (provider gone,
                        // token expired, deleted upload) — say so instead of
                        // silently doing nothing.
                        None => {
                            let mut error = queue.error;
                            error.set(Some(format!("Couldn't load “{clicked_title}” from its provider.")));
                        }
                    }
                });
            },
            oncontextmenu: {
                let entry = entry.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    let (sc, sp) = providers_for_context.clone();
                    let local_tracks = local.tracks.peek().clone();
                    let entry = entry.clone();
                    spawn(async move {
                        // Open the menu even when the entry no longer
                        // resolves anywhere — deleting a dead row is the
                        // main reason to right-click it.
                        let track = resolve_history_entry(sc, sp, &local_tracks, &entry)
                            .await
                            .unwrap_or_else(|| placeholder_track(&entry));
                        ctx.open_for_history(pos.x, pos.y, track, entry);
                    });
                }
            },
            div { class: "cover-card-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
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

/// Menu-only stand-in for a history row that resolves nowhere (provider
/// gone, deleted upload). Playback actions on it will surface an error;
/// "Remove from history" — the point of opening it — works regardless.
fn placeholder_track(entry: &HistoryEntry) -> Track {
    Track {
        uri: TrackUri(entry.track_uri.clone().unwrap_or_default()),
        provider: match entry.provider.as_str() {
            "Spotify" => ProviderId::Spotify,
            "SoundCloud" => ProviderId::SoundCloud,
            _ => ProviderId::Local,
        },
        title: entry.title.clone(),
        artists: vec![ArtistRef {
            uri: ArtistUri(String::new()),
            name: entry.artist.clone(),
        }],
        album: None,
        duration: std::time::Duration::ZERO,
        cover_url: entry.cover_url.clone(),
        mbid: None,
        added_at: None,
    }
}

async fn resolve_history_entry(
    sc: Arc<dyn Provider>,
    sp: Arc<dyn Provider>,
    local_tracks: &[Track],
    entry: &HistoryEntry,
) -> Option<Track> {
    let exact = match (entry.provider.as_str(), entry.track_uri.as_ref()) {
        ("Spotify", Some(uri)) => sp.track(&TrackUri(uri.clone())).await.ok(),
        ("SoundCloud", Some(uri)) => sc.track(&TrackUri(uri.clone())).await.ok(),
        // Local plays resolve against the scanned library — no network.
        ("Local", Some(uri)) => local_tracks.iter().find(|t| t.uri.0 == *uri).cloned(),
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
        "Local" => local_tracks
            .iter()
            .find(|t| t.title == entry.title && t.artists.iter().any(|a| a.name == entry.artist))
            .cloned(),
        _ => None,
    }
}

#[component]
pub(super) fn RecentlyLikedRail(library: UseLibrary) -> Element {
    let tracks = library.recently_liked.read().clone();
    let is_loading = *library.is_loading.read();
    let error = library.error.read().clone();
    let context_ctx = TrackCtx::new(tracks.clone());

    rsx! {
        Rail { eyebrow: "Activity".to_string(), title: "Recently liked".to_string(),
            if tracks.is_empty() && error.is_some() {
                div { class: "home-error", "{error.as_deref().unwrap_or_default()}" }
            } else if tracks.is_empty() && is_loading {
                SkeletonRow {}
            } else if tracks.is_empty() {
                EmptyState {
                    icon: "fa-solid fa-heart",
                    title: "No liked songs yet.",
                    body: "Connect Spotify in Settings — your liked-songs library shows up here once the first sync finishes.",
                }
            } else {
                div { class: "home-rail-row",
                    for (idx, track) in tracks.iter().enumerate() {
                        TrackCard {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            tracks: context_ctx.clone(),
                            index: idx,
                        }
                    }
                }
            }
        }
    }
}
