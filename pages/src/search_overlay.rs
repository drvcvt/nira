//! Global search overlay.
//!
//! Opens from the shell via Ctrl+F / Cmd+F or Alt+Space. It reuses the
//! existing multi-provider `use_search` hook but renders as a modal command
//! surface instead of a full sidebar page.

use components::SearchBar;
use dioxus::prelude::*;
use hooks::{use_detail, use_search};

use crate::parts::{ArtistResults, PlaylistResults, SearchTrackRow, TrackCtx};

#[component]
pub fn SearchOverlay(mut open: Signal<bool>, on_search: EventHandler<()>) -> Element {
    let mut search = use_search();
    let detail = use_detail();
    let is_open = *open.read();

    // Hand focus back on close. Closing reverts the overlay to
    // `visibility: hidden`, which blurs the focused input to <body> — and a
    // focused body means the shell's Rust onkeydown stops receiving anything.
    use_effect(move || {
        components::overlay_focus(*open.read(), ".search-overlay.open .searchbar-input");
    });

    if !is_open {
        return rsx! { div { class: "search-overlay" } };
    }

    let query = search.query.read().clone();
    let results = search.results.read().clone();
    let artist_hits = search.artists.read().clone();
    let playlists = search.playlists.read().clone();
    let is_searching = *search.is_searching.read();
    let error = search.error.read().clone();
    let has_query = !query.trim().is_empty();
    let row_ctx = TrackCtx::new(results.clone());

    rsx! {
        div {
            class: "search-overlay open",
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
                "aria-hidden": "true",
                onclick: move |_| open.set(false),
            }
            div { class: "search-overlay-panel",
                div { class: "search-overlay-bar",
                    SearchBar {
                        key: "search-overlay-input-{is_open}",
                        icon: Some("fa-solid fa-magnifying-glass".to_string()),
                        value: query.clone(),
                        placeholder: "Search songs, artists, playlists, labels…".to_string(),
                        autofocus: is_open,
                        on_input: move |v: String| search.query.set(v),
                        on_submit: move |_| {
                            if !search.query.peek().trim().is_empty() {
                                detail.close();
                                open.set(false);
                                on_search.call(());
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
                            div { class: "search-overlay-empty-copy", "Results from your connected music providers. Enter opens the full page." }
                        }
                    } else if results.is_empty()
                        && artist_hits.is_empty()
                        && playlists.is_empty()
                        && !is_searching
                    {
                        div { class: "search-overlay-empty",
                            div { class: "search-overlay-empty-title", "No results" }
                            div { class: "search-overlay-empty-copy", "Try artist + title or a shorter query." }
                        }
                    } else {
                        ArtistResults {
                            artists: artist_hits,
                            on_open: move |_| open.set(false),
                        }
                        PlaylistResults {
                            playlists,
                            on_open: move |_| open.set(false),
                        }
                        ul { class: "search-overlay-list",
                            for (idx, track) in results.iter().enumerate() {
                                SearchTrackRow {
                                    key: "{track.uri.0}",
                                    track: track.clone(),
                                    tracks: row_ctx.clone(),
                                    index: idx,
                                    class: "search-overlay-row".to_string(),
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
