//! Library page — two tabs:
//!
//! - **Saved** — local likes stored on disk (cross-provider). Anything the
//!   user hearts in nira lands here, regardless of where it streams from.
//! - **Spotify Liked** — the Spotify-server-side liked songs list, pulled
//!   live via the API. Read-only mirror.

use dioxus::prelude::*;
use hooks::{LikedTrack, ProviderId, Track, use_ctx_menu, use_library, use_likes, use_queue};

use crate::parts::ArtistLinks;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibTab {
    Saved,
    Spotify,
}

#[component]
pub fn Library() -> Element {
    let library = use_library();
    let likes = use_likes();
    let queue = use_queue();

    let mut tab = use_signal(|| LibTab::Saved);
    let active = *tab.read();

    let saved = likes.list();
    let spotify_tracks = library.liked.read().clone();
    let is_loading = *library.is_loading.read();
    let lib_error = library.error.read().clone();
    let queue_error = queue.error.read().clone();
    let progress = *library.progress.read();

    rsx! {
        section { class: "page",
            h1 { "Library" }

            div { class: "lib-tabs",
                button {
                    class: if active == LibTab::Saved { "lib-tab active" } else { "lib-tab" },
                    onclick: move |_| tab.set(LibTab::Saved),
                    i { class: "fa-solid fa-heart" }
                    " Saved "
                    span { class: "lib-tab-count", "{saved.len()}" }
                }
                button {
                    class: if active == LibTab::Spotify { "lib-tab active" } else { "lib-tab" },
                    onclick: move |_| tab.set(LibTab::Spotify),
                    i { class: "fa-brands fa-spotify" }
                    " Spotify Liked "
                    span { class: "lib-tab-count", "{spotify_tracks.len()}" }
                }
            }

            match active {
                LibTab::Saved => rsx! { SavedList { items: saved } },
                LibTab::Spotify => rsx! {
                    SpotifyLikedList {
                        tracks: spotify_tracks,
                        is_loading,
                        lib_error: lib_error.clone(),
                        queue_error: queue_error.clone(),
                        progress,
                    }
                },
            }
        }
    }
}

#[component]
fn SavedList(items: Vec<LikedTrack>) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let likes = use_likes();

    if items.is_empty() {
        return rsx! {
            div { class: "discover-empty",
                div { class: "discover-empty-glyph",
                    i { class: "fa-regular fa-heart" }
                }
                p { "No saved songs yet." }
                p { class: "hint",
                    "Right-click any track and pick \"Save to Liked\" — or hit the heart in the player. They land here."
                }
            }
        };
    }

    let tracks: Vec<Track> = items.iter().map(|l| l.track.clone()).collect();

    rsx! {
        p { class: "hint", "{items.len()} tracks" }
        ul { class: "track-list",
            for (idx, entry) in items.iter().enumerate() {
                {
                    let track = entry.track.clone();
                    let liked_at = entry.liked_at;
                    let tracks = tracks.clone();
                    let queue = queue.clone();
                    let ctx = ctx;
                    let likes = likes;
                    let t_for_ctx = track.clone();
                    let t_for_unlike = track.clone();
                    rsx! {
                        TrackRow {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            saved_at: Some(liked_at),
                            show_unlike: true,
                            on_play: move |_| queue.play_list(tracks.clone(), idx),
                            on_unlike: move |_| likes.toggle(&t_for_unlike),
                            on_context: move |e: MouseEvent| {
                                e.prevent_default();
                                let pos = e.data.client_coordinates();
                                ctx.open(pos.x, pos.y, t_for_ctx.clone());
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SpotifyLikedList(
    tracks: Vec<Track>,
    is_loading: bool,
    lib_error: Option<String>,
    queue_error: Option<String>,
    progress: (u32, u32),
) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();

    rsx! {
        p { class: "hint",
            "Your Spotify-side Liked Songs. Click a track to play; the rest becomes your queue."
        }

        if is_loading {
            p { class: "hint",
                i { class: "fa-solid fa-circle-notch fa-spin" }
                {
                    let (loaded, total) = progress;
                    if total > 0 {
                        format!(" Loading {loaded} of {total}…")
                    } else {
                        " Loading…".to_string()
                    }
                }
            }
        }

        if let Some(err) = lib_error.as_ref().or(queue_error.as_ref()) {
            div { class: "search-error", "{err}" }
        }

        if !is_loading && lib_error.is_none() && tracks.is_empty() {
            div { class: "discover-empty",
                div { class: "discover-empty-glyph",
                    i { class: "fa-solid fa-heart" }
                }
                p { "No Spotify-liked songs yet." }
                p { class: "hint", "Connect Spotify and like a few tracks — they'll show up here." }
            }
        }

        if !tracks.is_empty() {
            p { class: "hint", "{tracks.len()} tracks" }
            ul { class: "track-list",
                for (idx, track) in tracks.iter().enumerate() {
                    {
                        let track = track.clone();
                        let tracks = tracks.clone();
                        let queue = queue.clone();
                        let ctx = ctx;
                        let t_for_ctx = track.clone();
                        rsx! {
                            TrackRow {
                                key: "{track.uri.0}",
                                track: track.clone(),
                                saved_at: None,
                                show_unlike: false,
                                on_play: move |_| queue.play_list(tracks.clone(), idx),
                                on_unlike: move |_| {},
                                on_context: move |e: MouseEvent| {
                                    e.prevent_default();
                                    let pos = e.data.client_coordinates();
                                    ctx.open(pos.x, pos.y, t_for_ctx.clone());
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TrackRow(
    track: Track,
    saved_at: Option<chrono::DateTime<chrono::Utc>>,
    show_unlike: bool,
    on_play: EventHandler<()>,
    on_unlike: EventHandler<()>,
    on_context: EventHandler<MouseEvent>,
) -> Element {
    let duration = fmt_duration(track.duration);
    let cover = track.cover_url.clone().unwrap_or_default();
    let badge_class = match track.provider {
        ProviderId::Spotify => "track-badge spotify",
        ProviderId::SoundCloud => "track-badge soundcloud",
        ProviderId::Local => "track-badge",
    };
    let saved_str = saved_at.map(fmt_relative).unwrap_or_default();

    rsx! {
        li { class: "track-row",
            onclick: move |_| on_play.call(()),
            oncontextmenu: move |e: MouseEvent| on_context.call(e),
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
            if !saved_str.is_empty() {
                div { class: "track-saved-at", "{saved_str}" }
            }
            div { class: "track-duration", "{duration}" }
            div { class: "{badge_class}", "{track.provider.badge()}" }
            if show_unlike {
                button {
                    class: "track-row-unlike",
                    title: "Remove from Liked",
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        on_unlike.call(());
                    },
                    i { class: "fa-solid fa-heart" }
                }
            }
        }
    }
}

fn fmt_duration(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

fn fmt_relative(t: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let elapsed = now.signed_duration_since(t);
    let s = elapsed.num_seconds();
    if s < 60 {
        "just now".into()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else if s < 86_400 * 30 {
        format!("{}d ago", s / 86_400)
    } else {
        t.format("%Y-%m-%d").to_string()
    }
}
