use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{PlaylistBrief, use_detail, use_playlist, use_queue};

use crate::parts::{SearchTrackRow, TrackCtx};

#[component]
pub fn PlaylistPage(brief: PlaylistBrief) -> Element {
    let detail = use_detail();
    let queue = use_queue();
    let playlist = use_playlist();
    let uri = brief.uri.clone();

    use_effect(use_reactive!(|uri| {
        playlist.load(uri);
    }));

    let tracks = playlist.tracks.read().clone();
    let is_loading = *playlist.is_loading.read();
    let error = playlist.error.read().clone();
    let row_ctx = TrackCtx::new(tracks.clone());
    let provider = brief.provider.label().to_lowercase();
    let eyebrow = format!("{} · {provider}", brief.kind.label());
    let mut byline = Vec::new();
    if let Some(owner) = brief.owner_name.as_deref() {
        byline.push(format!("by {owner}"));
    }
    if let Some(count) = brief.track_count {
        byline.push(format!(
            "{count} {}",
            if count == 1 { "track" } else { "tracks" }
        ));
    }
    let byline = byline.join(" · ");
    let cover = brief.cover_url.clone().unwrap_or_default();
    let play_tracks = tracks.clone();

    rsx! {
        section { class: "page playlist-page",
            div { class: "artist-nav",
                Button {
                    label: "Back".to_string(),
                    icon: Some("fa-solid fa-arrow-left".to_string()),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: move |_| detail.back(),
                }
            }
            header { class: "playlist-banner",
                div { class: "banner-cover",
                    if !cover.is_empty() {
                        img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
                    } else {
                        span { class: "banner-cover-fallback",
                            i { class: "fa-solid fa-list-music" }
                        }
                    }
                }
                div { class: "banner-meta",
                    span { class: "banner-eyebrow", "{eyebrow}" }
                    h2 { class: "banner-name", "{brief.title}" }
                    if !byline.is_empty() {
                        div { class: "album-byline", "{byline}" }
                    }
                    div { class: "banner-actions",
                        Button {
                            label: "Play".to_string(),
                            icon: Some("fa-solid fa-play".to_string()),
                            variant: ButtonVariant::Primary,
                            disabled: tracks.is_empty(),
                            on_click: move |_| {
                                if !play_tracks.is_empty() {
                                    queue.play_context(play_tracks.clone(), 0);
                                }
                            },
                        }
                    }
                }
            }
            if is_loading {
                div { class: "discover-empty",
                    i { class: "fa-solid fa-circle-notch fa-spin" }
                    p { "Loading playlist…" }
                }
            } else if let Some(err) = error.as_ref() {
                div { class: "search-error", "Couldn't load playlist: {err}" }
            } else if tracks.is_empty() {
                div { class: "discover-empty",
                    p { "This playlist has no playable tracks." }
                }
            } else {
                ul { class: "track-list playlist-track-list",
                    for (index, track) in tracks.iter().enumerate() {
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
