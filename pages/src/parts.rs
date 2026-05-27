//! Small render-only helpers shared across pages.
//!
//! Lives at the page layer (not in `components/`) because everything
//! here is page-scoped chrome — track rows, inline links — and pages
//! already depend on `hooks`, so no extra dep wiring is needed.

use dioxus::prelude::*;
use hooks::{
    AlbumRef, ArtistRef, ProviderId, Track, UseCtxMenu, use_ctx_menu, use_detail, use_queue,
};

pub fn format_duration(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

pub fn provider_badge_class(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Spotify => "track-badge spotify",
        ProviderId::SoundCloud => "track-badge soundcloud",
        ProviderId::Local => "track-badge",
    }
}

pub fn open_track_context(ctx: UseCtxMenu, e: Event<MouseData>, track: Track) {
    e.prevent_default();
    let pos = e.data.client_coordinates();
    ctx.open(pos.x, pos.y, track);
}

#[component]
pub fn PlayableLi(
    track: Track,
    tracks: Vec<Track>,
    index: usize,
    class: String,
    #[props(default)] title: String,
    #[props(default)] on_played: Option<EventHandler<()>>,
    children: Element,
) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let play_tracks = tracks.clone();
    let ctx_track = track.clone();

    rsx! {
        li {
            class: "{class}",
            title: "{title}",
            onclick: move |_| {
                queue.play_context(play_tracks.clone(), index);
                if let Some(cb) = on_played.as_ref() {
                    cb.call(());
                }
            },
            oncontextmenu: move |e: Event<MouseData>| open_track_context(ctx, e, ctx_track.clone()),
            {children}
        }
    }
}

#[component]
pub fn PlayableButton(
    track: Track,
    tracks: Vec<Track>,
    index: usize,
    class: String,
    #[props(default)] title: String,
    #[props(default)] on_played: Option<EventHandler<()>>,
    children: Element,
) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let play_tracks = tracks.clone();
    let ctx_track = track.clone();

    rsx! {
        button {
            class: "{class}",
            title: "{title}",
            r#type: "button",
            onclick: move |_| {
                queue.play_context(play_tracks.clone(), index);
                if let Some(cb) = on_played.as_ref() {
                    cb.call(());
                }
            },
            oncontextmenu: move |e: Event<MouseData>| open_track_context(ctx, e, ctx_track.clone()),
            {children}
        }
    }
}

/// Renders `track.artists` as a comma-separated list where each name is
/// a button that opens the artist detail page. Clicks stop propagation
/// so the surrounding row's `onclick` (typically Play) doesn't fire.
/// Artists without a URI render as plain spans.
#[component]
pub fn ArtistLinks(artists: Vec<ArtistRef>) -> Element {
    let detail = use_detail();
    rsx! {
        for (idx, artist) in artists.iter().enumerate() {
            if idx > 0 {
                span { class: "artist-link-sep", ", " }
            }
            if artist.uri.0.is_empty() {
                span { "{artist.name}" }
            } else {
                button {
                    class: "artist-link",
                    onclick: {
                        let uri = artist.uri.clone();
                        move |e: Event<MouseData>| {
                            e.stop_propagation();
                            detail.open_artist(uri.clone());
                        }
                    },
                    "{artist.name}"
                }
            }
        }
    }
}

/// Inline album-name button mirroring [`ArtistLinks`]. Pages that show
/// `Track.album` use this to give a one-click jump to the album page.
#[component]
pub fn AlbumLink(album: AlbumRef) -> Element {
    let detail = use_detail();
    let title = album.title.clone();
    if album.uri.0.is_empty() {
        rsx! { span { "{title}" } }
    } else {
        rsx! {
            button {
                class: "artist-link album-link",
                onclick: {
                    let uri = album.uri.clone();
                    move |e: Event<MouseData>| {
                        e.stop_propagation();
                        detail.open_album(uri.clone());
                    }
                },
                "{title}"
            }
        }
    }
}
