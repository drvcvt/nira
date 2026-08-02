use components::SearchBar;
use dioxus::prelude::*;
use hooks::use_search;

use crate::parts::{ArtistResults, PlaylistResults, SearchTrackRow, TrackCtx};

#[component]
pub fn Search() -> Element {
    let mut search = use_search();
    let results = search.results.read().clone();
    let artist_hits = search.artists.read().clone();
    let playlists = search.playlists.read().clone();
    let is_searching = *search.is_searching.read();
    let error = search.error.read().clone();
    let query = search.query.read().clone();
    let has_query = !query.trim().is_empty();
    let row_ctx = TrackCtx::new(results.clone());

    rsx! {
        section { class: "page search-page",
            h1 { "Search" }
            p { class: "hint", "Results from your connected music providers." }

            SearchBar {
                icon: Some("fa-solid fa-magnifying-glass".to_string()),
                value: query,
                placeholder: "Search songs, artists, playlists, labels…".to_string(),
                on_input: move |value: String| search.query.set(value),
                autofocus: true,
                if is_searching && has_query {
                    span { class: "search-hint searching",
                        i { class: "fa-solid fa-circle-notch fa-spin" }
                        " searching"
                    }
                }
            }

            if has_query {
                if let Some(message) = error.as_ref() {
                    div { class: "search-error", "{message}" }
                } else if results.is_empty()
                    && artist_hits.is_empty()
                    && playlists.is_empty()
                    && !is_searching
                {
                    div { class: "search-empty", "No results." }
                }

                ArtistResults { artists: artist_hits }
                PlaylistResults { playlists }

                ul { class: "track-list",
                    for (index, track) in results.iter().enumerate() {
                        SearchTrackRow {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            tracks: row_ctx.clone(),
                            index,
                            class: "track-row".to_string(),
                        }
                    }
                }
            }
        }
    }
}
