//! Library page — two tabs:
//!
//! - **Saved** — local likes stored on disk (cross-provider). Anything the
//!   user hearts in nira lands here, regardless of where it streams from.
//! - **Spotify Liked** — the Spotify-server-side liked songs list, pulled
//!   live via the API. Read-only mirror.

use std::sync::Arc;

use dioxus::prelude::*;
use hooks::{LikedTrack, Track, use_ctx_menu, use_library, use_likes, use_queue};

use crate::parts::{ArtistLinks, format_duration, open_track_context, provider_badge_class};

const LIKED_PAGE_SIZE: usize = 150;

/// Shared playback context for a list — the click handler on every row
/// needs the full track vector so the queue gets the surrounding tracks
/// as upcoming items. Wrapping the vec in `Arc` (with pointer equality
/// for PartialEq) lets us pass the context as a Dioxus prop to N rows
/// without re-cloning or re-comparing the underlying vec.
#[derive(Clone)]
struct TrackContext(Arc<Vec<Track>>);

impl PartialEq for TrackContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl TrackContext {
    fn new(tracks: Vec<Track>) -> Self {
        Self(Arc::new(tracks))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn iter(&self) -> std::slice::Iter<'_, Track> {
        self.0.iter()
    }

    fn to_vec(&self) -> Vec<Track> {
        (*self.0).clone()
    }
}

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
    // Wrap the (potentially ~900-track) Spotify-liked list in an Arc once
    // per render. Without this we'd hand a `Vec<Track>` to SpotifyLikedList,
    // which Dioxus diffs by `PartialEq` — a full O(N) walk — *and* the
    // component cloned the vec again internally. Arc-as-PartialEq is
    // pointer equality, so a no-op render is free.
    let spotify_context = use_memo(move || TrackContext::new(library.liked.read().clone()));
    let spotify_count = spotify_context.read().len();
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
                    span { class: "lib-tab-count", "{spotify_count}" }
                }
            }

            match active {
                LibTab::Saved => rsx! { SavedList { items: saved } },
                LibTab::Spotify => rsx! {
                    SpotifyLikedList {
                        context: spotify_context.read().clone(),
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
    let context = TrackContext::new(tracks);

    rsx! {
        p { class: "hint", "{items.len()} tracks" }
        ul { class: "track-list",
            for (idx, entry) in items.iter().enumerate() {
                {
                    let track = entry.track.clone();
                    let liked_at = entry.liked_at;
                    let context = context.clone();
                    let t_for_unlike = track.clone();
                    rsx! {
                        TrackRow {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            saved_at: Some(liked_at),
                            show_unlike: true,
                            context,
                            index: idx,
                            on_unlike: move |_| likes.toggle(&t_for_unlike),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SpotifyLikedList(
    context: TrackContext,
    is_loading: bool,
    lib_error: Option<String>,
    queue_error: Option<String>,
    progress: (u32, u32),
) -> Element {
    let total = context.len();
    let mut visible_count = use_signal(|| LIKED_PAGE_SIZE);
    let visible = (*visible_count.read()).min(total);

    rsx! {
        p { class: "hint",
            "Your Spotify-side Liked Songs. Click a track to play; the rest becomes your queue."
        }

        if is_loading {
            p { class: "hint",
                i { class: "fa-solid fa-circle-notch fa-spin" }
                {
                    let (loaded, total_p) = progress;
                    if total_p > 0 {
                        format!(" Loading {loaded} of {total_p}…")
                    } else {
                        " Loading…".to_string()
                    }
                }
            }
        }

        if total == 0 {
            if let Some(err) = lib_error.as_ref().or(queue_error.as_ref()) {
                div { class: "search-error", "{err}" }
            }
        }

        if !is_loading && lib_error.is_none() && total == 0 {
            div { class: "discover-empty",
                div { class: "discover-empty-glyph",
                    i { class: "fa-solid fa-heart" }
                }
                p { "No Spotify-liked songs yet." }
                p { class: "hint", "Connect Spotify and like a few tracks — they'll show up here." }
            }
        }

        if total > 0 {
            p { class: "hint", "Showing {visible} of {total} tracks" }
            ul { class: "track-list",
                for (idx, track) in context.iter().take(visible).enumerate() {
                    {
                        let track = track.clone();
                        let row_context = context.clone();
                        rsx! {
                            TrackRow {
                                key: "{track.uri.0}",
                                track: track.clone(),
                                saved_at: None,
                                show_unlike: false,
                                context: row_context,
                                index: idx,
                                on_unlike: move |_| {},
                            }
                        }
                    }
                }
            }
            if visible < total {
                button {
                    class: "sq-btn sq-btn-ghost sq-sm library-more-btn",
                    onclick: move |_| visible_count.set((visible + LIKED_PAGE_SIZE).min(total)),
                    "Show more"
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
    context: TrackContext,
    index: usize,
    on_unlike: EventHandler<()>,
) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let duration = format_duration(track.duration);
    let cover = track.cover_url.clone().unwrap_or_default();
    let badge_class = provider_badge_class(track.provider);
    let saved_str = saved_at.map(fmt_relative).unwrap_or_default();
    let play_context = context.clone();
    let ctx_track = track.clone();

    rsx! {
        li {
            class: "track-row",
            onclick: move |_| queue.play_context(play_context.to_vec(), index),
            oncontextmenu: move |e: Event<MouseData>| open_track_context(ctx, e, ctx_track.clone()),
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
