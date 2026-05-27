//! Global search overlay.
//!
//! Opens from the shell via Ctrl+F / Cmd+F or Alt+Space. It reuses the
//! existing multi-provider `use_search` hook but renders as a modal command
//! surface instead of a full sidebar page.

use components::SearchBar;
use dioxus::prelude::*;
use hooks::{Track, use_queue, use_search};

use crate::parts::{ArtistLinks, PlayableLi, format_duration, provider_badge_class};

#[component]
pub fn SearchOverlay(mut open: Signal<bool>) -> Element {
    let mut search = use_search();
    let queue = use_queue();
    let is_open = *open.read();
    let query = search.query.read().clone();
    let results = search.results.read().clone();
    let is_searching = *search.is_searching.read();
    let error = search.error.read().clone();
    let has_query = !query.trim().is_empty();

    let overlay_class = if is_open {
        "search-overlay open"
    } else {
        "search-overlay"
    };

    rsx! {
        div {
            class: "{overlay_class}",
            onkeydown: move |e: Event<KeyboardData>| {
                if e.key() == Key::Escape {
                    e.prevent_default();
                    open.set(false);
                }
            },
            button {
                class: "search-overlay-backdrop",
                r#type: "button",
                tabindex: "-1",
                onclick: move |_| open.set(false),
            }
            div { class: "search-overlay-panel",
                div { class: "search-overlay-bar",
                    SearchBar {
                        key: "search-overlay-input-{is_open}",
                        icon: Some("fa-solid fa-magnifying-glass".to_string()),
                        value: query.clone(),
                        placeholder: "Search songs, artists, labels…".to_string(),
                        autofocus: is_open,
                        on_input: move |v: String| search.query.set(v),
                        on_submit: {
                            let queue = queue.clone();
                            let results = results.clone();
                            move |_| {
                                if !results.is_empty() {
                                    queue.play_list(results.clone(), 0);
                                    open.set(false);
                                }
                            }
                        },
                        span { class: "overlay-search-hint",
                            if is_searching {
                                i { class: "fa-solid fa-circle-notch fa-spin" }
                                " searching"
                            } else {
                                kbd { "Esc" }
                                " close"
                            }
                        }
                    }
                }

                div { class: "search-overlay-results",
                    if let Some(msg) = error.as_ref() {
                        div { class: "search-error", "{msg}" }
                    } else if !has_query {
                        div { class: "search-overlay-empty",
                            div { class: "search-overlay-empty-title", "Start typing" }
                            div { class: "search-overlay-empty-copy", "SoundCloud first, Spotify when connected. Enter plays the first result." }
                        }
                    } else if results.is_empty() && !is_searching {
                        div { class: "search-overlay-empty",
                            div { class: "search-overlay-empty-title", "No results" }
                            div { class: "search-overlay-empty-copy", "Try artist + title or a shorter query." }
                        }
                    } else {
                        ul { class: "search-overlay-list",
                            for (idx, track) in results.iter().enumerate() {
                                OverlayTrackRow {
                                    key: "{track.uri.0}",
                                    track: track.clone(),
                                    tracks: results.clone(),
                                    index: idx,
                                    on_played: move |_| open.set(false),
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
fn OverlayTrackRow(
    track: Track,
    tracks: Vec<Track>,
    index: usize,
    on_played: EventHandler<()>,
) -> Element {
    let duration = format_duration(track.duration);
    let cover = track.cover_url.clone().unwrap_or_default();
    let badge_class = provider_badge_class(track.provider);

    rsx! {
        PlayableLi {
            track: track.clone(),
            tracks,
            index,
            class: "search-overlay-row".to_string(),
            on_played,
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
