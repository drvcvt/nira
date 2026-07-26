//! Small render-only helpers shared across pages.
//!
//! Lives at the page layer (not in `components/`) because everything
//! here is page-scoped chrome — track rows, inline links — and pages
//! already depend on `hooks`, so no extra dep wiring is needed.

use dioxus::prelude::*;
use hooks::{
    AlbumRef, Artist, ArtistRef, ProviderId, Track, UseCtxMenu, uri_has_detail_page, use_ctx_menu,
    use_detail, use_queue,
};

pub fn format_duration(d: std::time::Duration) -> String {
    hooks::fmt_time(d.as_secs())
}

/// Shared playback context for a list of rows. Every row's click handler
/// needs the surrounding track vector (the queue takes it as upcoming
/// items), but handing each row its own `Vec<Track>` clone made an N-row
/// list cost O(N²) track clones per render — plus a full O(N) PartialEq
/// walk per row prop on every diff. `Arc` + pointer-equality makes the
/// share free; build it ONCE per list render and pass `ctx.clone()` to
/// each row.
#[derive(Clone)]
pub struct TrackCtx(std::sync::Arc<Vec<Track>>);

impl PartialEq for TrackCtx {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl TrackCtx {
    pub fn new(tracks: Vec<Track>) -> Self {
        Self(std::sync::Arc::new(tracks))
    }

    /// Materialize for the queue — the one place that needs ownership,
    /// and only on click, not per render.
    pub fn to_vec(&self) -> Vec<Track> {
        (*self.0).clone()
    }
}

/// Normalized (artist, title) index of the on-disk library, shared between
/// the album header's owned-count and the row list's per-row check.
///
/// Same `Arc` + pointer-equality trick as [`TrackCtx`], for the same reason:
/// `track_match_key` costs 8 allocations per library track, both components
/// were building their own copy every render, and both stay subscribed to
/// `local.tracks` while hidden under the detail stack — so one rescan
/// rebuilt the index once per mounted album page, twice over.
#[derive(Clone)]
pub struct OwnedIndex(std::sync::Arc<std::collections::HashSet<(String, String)>>);

impl PartialEq for OwnedIndex {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl OwnedIndex {
    pub fn new(keys: std::collections::HashSet<(String, String)>) -> Self {
        Self(std::sync::Arc::new(keys))
    }

    pub fn contains(&self, key: &(String, String)) -> bool {
        self.0.contains(key)
    }
}

pub fn provider_badge_class(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Spotify => "track-badge spotify",
        ProviderId::SoundCloud => "track-badge soundcloud",
        ProviderId::Local => "track-badge",
    }
}

/// Artist search hits — avatar-pill row shared by the Search page and the
/// global search overlay. Click opens the artist detail view; `on_open`
/// lets the overlay close itself after navigating.
#[component]
pub fn ArtistResults(
    artists: Vec<Artist>,
    #[props(default)] on_open: Option<EventHandler<()>>,
) -> Element {
    let detail = use_detail();
    if artists.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "artist-results",
            for a in artists.iter() {
                button {
                    key: "{a.uri.0}",
                    class: "artist-result",
                    r#type: "button",
                    title: "{a.name} · {a.provider.label()}",
                    onclick: {
                        let uri = a.uri.clone();
                        move |_| {
                            detail.open_artist(uri.clone());
                            if let Some(cb) = on_open.as_ref() {
                                cb.call(());
                            }
                        }
                    },
                    span { class: "artist-result-avatar",
                        if let Some(img) = a.image_url.as_ref() {
                            img { src: "{img}", alt: "", loading: "lazy", decoding: "async" }
                        } else {
                            i { class: "fa-solid fa-user" }
                        }
                    }
                    span { class: "artist-result-name", "{a.name}" }
                    span { class: "artist-result-badge", "{a.provider.badge()}" }
                }
            }
        }
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
    tracks: TrackCtx,
    index: usize,
    class: String,
    #[props(default)] title: String,
    #[props(default)] on_played: Option<EventHandler<()>>,
    children: Element,
) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let play_tracks = tracks.clone();
    let key_tracks = tracks.clone();
    let key_queue = queue.clone();
    let ctx_track = track.clone();

    rsx! {
        li {
            class: "{class}",
            title: "{title}",
            // Keyboard path: rows are plain <li>s, so opt them into the tab
            // order and mirror the click action on Enter.
            tabindex: "0",
            role: "button",
            onclick: move |_| {
                queue.play_context(play_tracks.to_vec(), index);
                if let Some(cb) = on_played.as_ref() {
                    cb.call(());
                }
            },
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Enter {
                    key_queue.play_context(key_tracks.to_vec(), index);
                    if let Some(cb) = on_played.as_ref() {
                        cb.call(());
                    }
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
    tracks: TrackCtx,
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
                queue.play_context(play_tracks.to_vec(), index);
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
            if !uri_has_detail_page(&artist.uri.0) {
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
                    // Keyboard Enter activates the button's click natively;
                    // without this stop the keydown ALSO bubbles to the
                    // surrounding PlayableLi and starts playback.
                    onkeydown: |e: KeyboardEvent| {
                        if e.key() == Key::Enter {
                            e.stop_propagation();
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
    if !uri_has_detail_page(&album.uri.0) {
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
                // Same Enter-bubbling guard as ArtistLinks.
                onkeydown: |e: KeyboardEvent| {
                    if e.key() == Key::Enter {
                        e.stop_propagation();
                    }
                },
                "{title}"
            }
        }
    }
}
