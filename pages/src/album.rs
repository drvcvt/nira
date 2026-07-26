//! Album detail page — cover, header, tracklist.


use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    AlbumCtx, AlbumUri, Track, track_match_key, use_album,
    use_ctx_menu, use_detail, use_local_library, use_queue,
};

use crate::parts::{OwnedIndex, PlayableLi, TrackCtx, format_duration};

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
    // One index build per rescan, shared by the header and the row list.
    let local = use_local_library();
    let owned = use_memo(move || {
        OwnedIndex::new(local.tracks.read().iter().map(track_match_key).collect())
    });

    rsx! {
        section { class: "page album-page",
            div { class: "artist-nav",
                Button {
                    label: "Back".to_string(),
                    icon: Some("fa-solid fa-arrow-left".to_string()),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: move |_| detail.back(),
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
                    owned: owned(),
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
                AlbumTrackList { tracks: d.tracks.clone(), owned: owned() }
            }
        }
    }
}

#[component]
fn AlbumHeader(
    detail: hooks::AlbumDetail,
    owned: OwnedIndex,
    on_play_all: EventHandler<()>,
) -> Element {
    let cover = detail.cover_url.clone().unwrap_or_default();
    let detail_router = use_detail();
    let ctx = use_ctx_menu();
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
    // How much of this album is already on disk. The index is memoized in
    // AlbumPage off the tracks signal, so this still updates live when a
    // download's rescan lands — without rebuilding it here.
    let owned_count = detail
        .tracks
        .iter()
        .filter(|t| owned.contains(&track_match_key(t)))
        .count();

    rsx! {
        header {
            class: "album-banner",
            // Album right-click: play/queue/add-to-playlist (as an album
            // widget) — same menu the Library album cards open.
            oncontextmenu: {
                let uri = detail.uri.0.clone();
                let title = detail.title.clone();
                let artist = detail.artist.name.clone();
                let cover_url = detail.cover_url.clone();
                let tracks = detail.tracks.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    ctx.open_album(
                        pos.x,
                        pos.y,
                        AlbumCtx {
                            uri: uri.clone(),
                            title: title.clone(),
                            artist: artist.clone(),
                            cover_url: cover_url.clone(),
                            tracks: tracks.clone(),
                        },
                    );
                }
            },
            div { class: "banner-cover",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
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
                    // Local artist "URIs" are grouping keys with no page —
                    // render plain text instead of a dead link.
                    if hooks::uri_has_detail_page(&artist_uri.0) {
                        button {
                            class: "artist-link-inline",
                            onclick: move |_| detail_router.open_artist(artist_uri.clone()),
                            "{artist_name}"
                        }
                    } else {
                        span { "{artist_name}" }
                    }
                    if !year_str.is_empty() {
                        span { class: "album-byline-sep", " · {year_str}" }
                    }
                    span { class: "album-byline-sep", " · {track_count} tracks" }
                    if owned_count > 0 && owned_count >= track_count {
                        span { class: "album-owned-chip",
                            i { class: "fa-solid fa-circle-check" }
                            "in library"
                        }
                    } else if owned_count > 0 {
                        span { class: "album-owned-chip",
                            i { class: "fa-solid fa-circle-half-stroke" }
                            "{owned_count}/{track_count} in library"
                        }
                    }
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
fn AlbumTrackList(tracks: Vec<Track>, owned: OwnedIndex) -> Element {
    if tracks.is_empty() {
        return rsx! {
            div { class: "discover-empty",
                p { "This release has no tracks." }
            }
        };
    }
    let row_ctx = TrackCtx::new(tracks.clone());
    rsx! {
        ul { class: "track-list",
            for (i, t) in tracks.iter().enumerate() {
                PlayableLi {
                    key: "{t.uri.0}",
                    track: t.clone(),
                    tracks: row_ctx.clone(),
                    index: i,
                    class: "track-row album-track-row".to_string(),
                    span { class: "track-index", "{i + 1:02}" }
                    div { class: "track-meta",
                        div { class: "track-title", "{t.title}" }
                        div { class: "track-artist",
                            "{t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(\", \")}"
                        }
                    }
                    div { class: "track-duration",
                        if owned.contains(&track_match_key(t)) {
                            i {
                                class: "fa-solid fa-check track-owned",
                                title: "In your library",
                            }
                        }
                        span { "{format_duration(t.duration)}" }
                    }
                }
            }
        }
    }
}
