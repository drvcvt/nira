//! Album detail page — cover, header, tracklist.

use std::collections::HashSet;

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    AlbumUri, ProviderId, Track, download_from_hires-provider_by_query, download_hires-provider_album,
    download_hires-provider_track, download_hires-provider_track_by_match, track_match_key, use_album, use_config,
    use_detail, use_downloads, use_local_library, use_hires-provider, use_queue,
};

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
    let qz = use_hires-provider();
    let downloads = use_downloads();
    let local = use_local_library();
    let config = use_config();
    let qz_connected = qz.is_connected();
    let is_hires-provider_album = detail.provider == ProviderId::the hi-res provider;
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
    // How much of this album is already on disk — reading the tracks signal
    // subscribes us, so the chip updates live when a download's rescan lands.
    let owned_count = {
        let local_tracks = local.tracks.read();
        let owned: HashSet<(String, String)> = local_tracks.iter().map(track_match_key).collect();
        detail
            .tracks
            .iter()
            .filter(|t| owned.contains(&track_match_key(t)))
            .count()
    };

    rsx! {
        header { class: "album-banner",
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
                    // Download as FLAC. A the hi-res provider album downloads directly; any
                    // other provider's album is matched on the hi-res provider first, so a
                    // Spotify/SC album can still be grabbed losslessly. The
                    // download is a delta — tracks already on disk are
                    // skipped, so the label counts only what's missing.
                    // Fully owned albums hide the button (the chip says why).
                    if qz_connected && owned_count < track_count {
                        Button {
                            label: if owned_count > 0 {
                                format!("Download {} missing (.flac)", track_count - owned_count)
                            } else {
                                "Download (.flac)".to_string()
                            },
                            icon: Some("fa-solid fa-download".to_string()),
                            variant: ButtonVariant::Ghost,
                            disabled: track_count == 0,
                            on_click: {
                                let qz = qz.clone();
                                let album_uri = detail.uri.0.clone();
                                let title = detail.title.clone();
                                let artist_name = detail.artist.name.clone();
                                move |_| {
                                    let root = config.read().library_root.clone();
                                    if is_hires-provider_album {
                                        download_hires-provider_album(qz.clone(), local.clone(), downloads, root, album_uri.clone(), title.clone());
                                    } else {
                                        download_from_hires-provider_by_query(qz.clone(), local.clone(), downloads, root, artist_name.clone(), title.clone());
                                    }
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AlbumTrackList(tracks: Vec<Track>) -> Element {
    let local = use_local_library();
    let qz = use_hires-provider();
    let downloads = use_downloads();
    let config = use_config();
    let qz_connected = qz.is_connected();
    if tracks.is_empty() {
        return rsx! {
            div { class: "discover-empty",
                p { "This release has no tracks." }
            }
        };
    }
    // Normalized (artist, title) index of the on-disk library — one build
    // per render, O(1) per row below.
    let owned: HashSet<(String, String)> =
        local.tracks.read().iter().map(track_match_key).collect();
    rsx! {
        ul { class: "track-list",
            for (i, t) in tracks.iter().enumerate() {
                PlayableLi {
                    key: "{t.uri.0}",
                    track: t.clone(),
                    tracks: tracks.clone(),
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
                        } else if qz_connected {
                            // Single-track grab. the hi-res provider tracks download
                            // directly; other providers go through the
                            // strict the hi-res provider match first.
                            button {
                                class: "track-dl",
                                r#type: "button",
                                title: "Download (.flac)",
                                onclick: {
                                    let qz = qz.clone();
                                    let local = local.clone();
                                    let t = t.clone();
                                    move |e: Event<MouseData>| {
                                        e.stop_propagation();
                                        let root = config.read().library_root.clone();
                                        if t.provider == ProviderId::the hi-res provider {
                                            download_hires-provider_track(
                                                qz.clone(), local.clone(), downloads, root,
                                                t.uri.0.clone(), t.title.clone(),
                                            );
                                        } else {
                                            download_hires-provider_track_by_match(
                                                qz.clone(), local.clone(), downloads, root,
                                                t.clone(),
                                            );
                                        }
                                    }
                                },
                                i { class: "fa-solid fa-download" }
                            }
                        }
                        span { "{format_duration(t.duration)}" }
                    }
                }
            }
        }
    }
}
