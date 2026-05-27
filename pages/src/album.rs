//! Album detail page — cover, header, tracklist.

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{AlbumUri, Track, use_album, use_detail, use_queue};

use crate::parts::{PlayableLi, format_duration};

#[component]
pub fn AlbumPage(uri: AlbumUri) -> Element {
    let detail = use_detail();
    let queue = use_queue();
    let album = use_album();

    use_effect(use_reactive!(|uri| {
        album.load(uri);
    }));

    let view = album.view.read().clone();
    let is_loading = *album.is_loading.read();
    let error = album.error.read().clone();

    rsx! {
        section { class: "page album-page",
            div { class: "artist-nav",
                Button {
                    label: "Back".to_string(),
                    icon: Some("fa-solid fa-arrow-left".to_string()),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: move |_| detail.close(),
                }
            }
            if is_loading {
                div { class: "discover-empty",
                    i { class: "fa-solid fa-circle-notch fa-spin" }
                    p { "Loading album…" }
                }
            } else if let Some(err) = error.as_ref() {
                div { class: "search-error", "Couldn't load album: {err}" }
            } else if let Some(d) = view.as_ref() {
                AlbumHeader {
                    detail: d.clone(),
                    on_play_all: {
                        let queue = queue.clone();
                        let tracks = d.tracks.clone();
                        move |_| {
                            if !tracks.is_empty() {
                                queue.play_context(tracks.clone(), 0);
                            }
                        }
                    }
                }
                AlbumTrackList { tracks: d.tracks.clone() }
            }
        }
    }
}

#[component]
fn AlbumHeader(detail: hooks::AlbumDetail, on_play_all: EventHandler<()>) -> Element {
    let cover = detail.cover_url.clone().unwrap_or_default();
    let detail_router = use_detail();
    let kind_label = match detail.album_type {
        hooks::AlbumType::Album => "album",
        hooks::AlbumType::Single => "single",
        hooks::AlbumType::Ep => "ep",
        hooks::AlbumType::Compilation => "compilation",
        hooks::AlbumType::Unknown => "release",
    };
    let year_str = detail
        .release_year
        .map(|y| y.to_string())
        .unwrap_or_default();
    let track_count = detail.tracks.len();
    let artist_name = detail.artist.name.clone();
    let artist_uri = detail.artist.uri.clone();
    let provider = detail.provider.label().to_lowercase();

    rsx! {
        header { class: "album-banner",
            div { class: "banner-cover",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    span { class: "banner-cover-fallback",
                        i { class: "fa-solid fa-compact-disc" }
                    }
                }
            }
            div { class: "banner-meta",
                span { class: "banner-eyebrow", "{kind_label} · {provider}" }
                h2 { class: "banner-name", "{detail.title}" }
                div { class: "album-byline",
                    "by "
                    button {
                        class: "artist-link-inline",
                        onclick: move |_| detail_router.open_artist(artist_uri.clone()),
                        "{artist_name}"
                    }
                    if !year_str.is_empty() {
                        span { class: "album-byline-sep", " · {year_str}" }
                    }
                    span { class: "album-byline-sep", " · {track_count} tracks" }
                }
                div { class: "banner-actions",
                    Button {
                        label: "Play".to_string(),
                        icon: Some("fa-solid fa-play".to_string()),
                        variant: ButtonVariant::Primary,
                        disabled: track_count == 0,
                        on_click: move |_| on_play_all.call(()),
                    }
                }
            }
        }
    }
}

#[component]
fn AlbumTrackList(tracks: Vec<Track>) -> Element {
    if tracks.is_empty() {
        return rsx! {
            div { class: "discover-empty",
                p { "This release has no tracks." }
            }
        };
    }
    rsx! {
        ul { class: "track-list",
            for (i, t) in tracks.iter().enumerate() {
                PlayableLi {
                    key: "{t.uri.0}",
                    track: t.clone(),
                    tracks: tracks.clone(),
                    index: i,
                    class: "track-row top-track-row".to_string(),
                    span { class: "track-index", "{i + 1:02}" }
                    div { class: "track-meta",
                        div { class: "track-title", "{t.title}" }
                        div { class: "track-artist",
                            "{t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(\", \")}"
                        }
                    }
                    div { class: "track-duration",
                        "{format_duration(t.duration)}"
                    }
                }
            }
        }
    }
}
