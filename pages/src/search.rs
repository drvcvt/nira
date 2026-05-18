use components::{SearchBar, SearchBarShape};
use dioxus::prelude::*;
use hooks::{ProviderId, Track, use_ctx_menu, use_queue, use_search};

use crate::parts::ArtistLinks;

#[component]
pub fn Search() -> Element {
    let mut search = use_search();
    let queue = use_queue();
    let results = search.results.read().clone();
    let is_searching = *search.is_searching.read();
    let is_loading_track = *queue.is_loading_track.read();
    let search_error = search.error.read().clone();
    let queue_error = queue.error.read().clone();
    let query_value = search.query.read().clone();
    let has_query = !query_value.trim().is_empty();

    rsx! {
        section { class: "page search-page",
            h1 { "Search" }
            p { class: "hint",
                "Across SoundCloud and Spotify (when connected). "
                "Click a track to stream — the rest of the list becomes the queue."
            }

            // Pill-shaped input with inline icon and a contextual hint
            // chip on the right (spinner → loading → enter hint).
            SearchBar {
                shape: SearchBarShape::Pill,
                icon: Some("fa-solid fa-magnifying-glass".to_string()),
                value: query_value.clone(),
                placeholder: "artist, title, label, anything…".to_string(),
                on_input: move |v: String| search.query.set(v),
                autofocus: true,
                if is_searching {
                    span { class: "search-hint searching",
                        i { class: "fa-solid fa-circle-notch fa-spin" }
                        " searching"
                    }
                } else if is_loading_track {
                    span { class: "search-hint loading",
                        i { class: "fa-solid fa-circle-notch fa-spin" }
                        " loading track"
                    }
                } else if has_query {
                    span { class: "search-hint",
                        kbd { class: "kbd", "⏎" }
                        " play first"
                    }
                }
            }

            if let Some(err) = search_error.as_ref().or(queue_error.as_ref()) {
                div { class: "search-error", "{err}" }
            }

            if results.is_empty() && !is_searching && has_query && search_error.is_none() {
                div { class: "search-empty", "No results." }
            }

            ul { class: "track-list",
                for (idx, track) in results.iter().enumerate() {
                    TrackRow {
                        key: "{track.uri.0}",
                        track: track.clone(),
                        on_play: {
                            let results = results.clone();
                            let queue = queue.clone();
                            move |_| queue.play_list(results.clone(), idx)
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn TrackRow(track: Track, on_play: EventHandler<()>) -> Element {
    let duration = format_duration(track.duration);
    let cover = track.cover_url.clone().unwrap_or_default();
    let badge_class = match track.provider {
        ProviderId::Spotify => "track-badge spotify",
        ProviderId::SoundCloud => "track-badge soundcloud",
        ProviderId::Local => "track-badge",
    };
    let ctx = use_ctx_menu();

    rsx! {
        li { class: "track-row",
            onclick: move |_| on_play.call(()),
            oncontextmenu: {
                let track = track.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    ctx.open(pos.x, pos.y, track.clone());
                }
            },
            div { class: "track-cover",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    div { class: "track-cover-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
            }
            div { class: "track-meta",
                div { class: "track-title", "{track.title}" }
                div { class: "track-artist",
                    ArtistLinks { artists: track.artists.clone() }
                }
            }
            div { class: "track-duration", "{duration}" }
            div { class: "{badge_class}", "{track.provider.badge()}" }
        }
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}
