//! Artist detail page — banner + tabs (Top / Albums / Singles / Related).
//!
//! Mounted by the shell when `use_detail` carries an Artist URI. Hides the
//! active Section's main content. Closes via the Back button at the top,
//! which clears `use_detail::current`.
//!
//! Single-provider for v1: the URI prefix decides which provider serves
//! the data. Cross-provider augmentation (showing SC tracks under a
//! Spotify artist, related artists across providers) lands later.

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    AlbumBrief, AlbumCtx, ArtistUri, RelatedArtist, Track, fetch_album_detail, is_long_play,
    use_artist, use_ctx_menu, use_detail, use_local_library, use_hires-provider, use_queue,
    use_soundcloud, use_spotify,
};

use crate::parts::{PlayableLi, TrackCtx};

#[component]
pub fn ArtistPage(uri: ArtistUri) -> Element {
    let detail = use_detail();
    let queue = use_queue();
    let artist = use_artist();

    // Kick off the load when the URI first lands; re-run if we ever
    // re-navigate to a different artist without remounting.
    use_effect(use_reactive!(|uri| {
        artist.load(uri);
    }));

    let view = artist.view.read().clone();
    let is_loading = *artist.is_loading.read();
    let error = artist.error.read().clone();
    let active_tab = use_signal(|| ArtistTab::Top);

    // Fetch Related whenever the tab is (or becomes) active for a view that
    // doesn't have it yet. Reactive instead of tab-click-triggered so it
    // also fires after navigating to another artist while Related was
    // already selected — the tab survives, the fresh view arrives with
    // `related: None`, and a click-only trigger would leave the spinner
    // spinning forever.
    use_effect(move || {
        let wants_related = *active_tab.read() == ArtistTab::Related;
        let missing = artist
            .view
            .read()
            .as_ref()
            .is_some_and(|v| v.related.is_none());
        if wants_related && missing && !*artist.is_loading_related.peek() {
            artist.load_related();
        }
    });

    rsx! {
        section { class: "page artist-page",
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
                ArtistSkeleton {}
            } else if let Some(err) = error.as_ref() {
                div { class: "search-error", "Couldn't load artist: {err}" }
            } else if let Some(v) = view.as_ref() {
                ArtistBanner {
                    view: v.clone(),
                    on_play_all: {
                        let queue = queue.clone();
                        let tracks = v.top_tracks.clone();
                        move |_| {
                            if !tracks.is_empty() {
                                queue.play_context(tracks.clone(), 0);
                            }
                        }
                    }
                }
                ArtistTabs {
                    view: v.clone(),
                    active: active_tab,
                }
                ArtistTabBody {
                    view: v.clone(),
                    active: active_tab,
                    is_loading_related: *artist.is_loading_related.read(),
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArtistTab {
    Top,
    Albums,
    Singles,
    Related,
}

impl ArtistTab {
    fn label(self) -> &'static str {
        match self {
            ArtistTab::Top => "Top Tracks",
            ArtistTab::Albums => "Albums",
            ArtistTab::Singles => "Singles & EPs",
            ArtistTab::Related => "Related",
        }
    }
}

#[component]
fn ArtistSkeleton() -> Element {
    rsx! {
        div { class: "artist-banner skeleton",
            div { class: "banner-cover skeleton-block" }
            div { class: "banner-meta",
                div { class: "skeleton-line wide" }
                div { class: "skeleton-line" }
                div { class: "skeleton-line short" }
            }
        }
    }
}

#[component]
fn ArtistBanner(view: hooks::ArtistView, on_play_all: EventHandler<()>) -> Element {
    let artist = view.artist.clone();
    let cover = artist.image_url.clone().unwrap_or_default();
    let provider = artist.provider.label().to_string();
    let track_count = view.top_tracks.len();
    let release_count = view.albums.len();
    let genres: Vec<String> = artist.genres.iter().take(5).cloned().collect();
    let permalink = artist.permalink_url.clone();
    // Honest source label — augmented views carry Spotify's catalogue.
    let source_label = if view.via_spotify.is_some() {
        format!("{} + spotify", provider.to_lowercase())
    } else {
        provider.to_lowercase()
    };

    rsx! {
        header { class: "artist-banner",
            div { class: "banner-cover",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
                } else {
                    span { class: "banner-cover-fallback",
                        i { class: "fa-solid fa-user" }
                    }
                }
            }
            div { class: "banner-meta",
                span { class: "banner-eyebrow", "artist · {provider.to_lowercase()}" }
                h2 { class: "banner-name", "{artist.name}" }
                if !genres.is_empty() {
                    div { class: "banner-genres",
                        for g in genres.iter() {
                            span { class: "banner-genre-chip", "{g}" }
                        }
                    }
                }
                dl { class: "banner-stats",
                    div { class: "banner-stat",
                        dt { "tracks" }
                        dd { class: "banner-stat-num", "{track_count}" }
                    }
                    div { class: "banner-stat",
                        dt { "releases" }
                        dd { class: "banner-stat-num", "{release_count}" }
                    }
                    div { class: "banner-stat",
                        dt { "source" }
                        dd { class: "banner-stat-mix", "{source_label}" }
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
                    if let Some(url) = permalink.as_ref() {
                        a {
                            class: "banner-link",
                            href: "{url}",
                            target: "_blank",
                            rel: "noopener",
                            "open in {provider.to_lowercase()} ↗"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ArtistTabs(view: hooks::ArtistView, active: Signal<ArtistTab>) -> Element {
    let mut active = active;
    let tabs = [
        (ArtistTab::Top, view.top_tracks.len()),
        (
            ArtistTab::Albums,
            view.albums.iter().filter(|a| is_long_play(a)).count(),
        ),
        (
            ArtistTab::Singles,
            view.albums.iter().filter(|a| !is_long_play(a)).count(),
        ),
        (ArtistTab::Related, 0),
    ];

    rsx! {
        div { class: "artist-tabs", role: "tablist",
            for (tab, count) in tabs.iter().copied() {
                button {
                    class: if *active.read() == tab { "artist-tab active" } else { "artist-tab" },
                    role: "tab",
                    // role="tab" without aria-selected is worse than a plain
                    // button — every tab announces identically.
                    "aria-selected": if *active.read() == tab { "true" } else { "false" },
                    onclick: move |_| active.set(tab),
                    span { class: "artist-tab-label", "{tab.label()}" }
                    if count > 0 && tab != ArtistTab::Related {
                        span { class: "artist-tab-count", "{count}" }
                    }
                }
            }
        }
    }
}

#[component]
fn ArtistTabBody(
    view: hooks::ArtistView,
    active: Signal<ArtistTab>,
    is_loading_related: bool,
) -> Element {
    match *active.read() {
        ArtistTab::Top => rsx! { TopTracksList { tracks: view.top_tracks.clone() } },
        ArtistTab::Albums => {
            let albums: Vec<AlbumBrief> = view
                .albums
                .iter()
                .filter(|a| is_long_play(a))
                .cloned()
                .collect();
            rsx! { AlbumGrid { albums, empty_label: "No full-length releases here yet." } }
        }
        ArtistTab::Singles => {
            let albums: Vec<AlbumBrief> = view
                .albums
                .iter()
                .filter(|a| !is_long_play(a))
                .cloned()
                .collect();
            rsx! { AlbumGrid { albums, empty_label: "No singles or EPs surfaced." } }
        }
        ArtistTab::Related => rsx! {
            RelatedGrid {
                related: view.related.clone().unwrap_or_default(),
                is_loading: is_loading_related,
                already_loaded: view.related.is_some(),
            }
        },
    }
}

#[component]
fn TopTracksList(tracks: Vec<Track>) -> Element {
    if tracks.is_empty() {
        return rsx! {
            div { class: "discover-empty",
                p { "No tracks surfaced for this artist yet." }
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
                    class: "track-row top-track-row".to_string(),
                    span { class: "track-index", "{i + 1:02}" }
                    div { class: "track-cover",
                        if let Some(c) = t.cover_url.as_ref() {
                            img { src: "{c}", alt: "", loading: "lazy", decoding: "async" }
                        } else {
                            div { class: "track-cover-fallback",
                                i { class: "fa-solid fa-music" }
                            }
                        }
                    }
                    div { class: "track-meta",
                        div { class: "track-title", "{t.title}" }
                        div { class: "track-artist",
                            if let Some(a) = t.album.as_ref() {
                                "{a.title}"
                            } else {
                                "—"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AlbumGrid(albums: Vec<AlbumBrief>, empty_label: String) -> Element {
    let detail = use_detail();
    if albums.is_empty() {
        return rsx! {
            div { class: "discover-empty",
                p { "{empty_label}" }
            }
        };
    }
    rsx! {
        div { class: "album-grid",
            for a in albums.iter() {
                AlbumCard {
                    key: "{a.uri.0}",
                    album: a.clone(),
                    on_open: {
                        let uri = a.uri.clone();
                        move |_| detail.open_album(uri.clone())
                    }
                }
            }
        }
    }
}

#[component]
fn AlbumCard(album: AlbumBrief, on_open: EventHandler<()>) -> Element {
    let ctx = use_ctx_menu();
    let sc = use_soundcloud();
    let sp = use_spotify();
    let qz = use_hires-provider();
    let local = use_local_library();
    let cover = album.cover_url.clone().unwrap_or_default();
    let provider = album.provider.label();
    let kind_label = match album.album_type {
        hooks::AlbumType::Album => "album".to_string(),
        hooks::AlbumType::Single => "single".to_string(),
        hooks::AlbumType::Ep => "ep".to_string(),
        hooks::AlbumType::Compilation => "compilation".to_string(),
        hooks::AlbumType::Unknown => "release".to_string(),
    };
    let year_str = album
        .release_year
        .map(|y| y.to_string())
        .unwrap_or_default();
    let tracks_str = album
        .total_tracks
        .map(|t| format!("{t} tracks"))
        .unwrap_or_default();
    let badge = provider.chars().next().unwrap_or('?').to_string();

    rsx! {
        button {
            class: "album-card",
            r#type: "button",
            onclick: move |_| on_open.call(()),
            // The brief card doesn't carry the track list — open the menu
            // instantly with what we have (track-dependent entries disable
            // themselves while empty) and land the tracks async. A fetch
            // failure leaves "Go to album" usable instead of a dead click.
            oncontextmenu: {
                let album = album.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    ctx.open_album(
                        pos.x,
                        pos.y,
                        AlbumCtx {
                            uri: album.uri.0.clone(),
                            title: album.title.clone(),
                            artist: album.artist_name.clone(),
                            cover_url: album.cover_url.clone(),
                            tracks: Vec::new(),
                        },
                    );
                    let (sc, sp, qz) = (sc.clone(), sp.clone(), qz.clone());
                    let uri = album.uri.clone();
                    spawn(async move {
                        if let Some(d) = fetch_album_detail(sc, sp, qz, local, uri).await {
                            ctx.set_album_tracks(&d.uri.0, d.tracks);
                        }
                    });
                }
            },
            div { class: "album-cover",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
                } else {
                    span { class: "album-cover-fallback",
                        i { class: "fa-solid fa-compact-disc" }
                    }
                }
                span { class: "provider-badge", title: "{provider}", "{badge}" }
            }
            div { class: "album-meta",
                span { class: "album-title", "{album.title}" }
                span { class: "album-sub",
                    "{kind_label}"
                    if !year_str.is_empty() { " · {year_str}" }
                    if !tracks_str.is_empty() { " · {tracks_str}" }
                }
            }
        }
    }
}

#[component]
fn RelatedGrid(related: Vec<RelatedArtist>, is_loading: bool, already_loaded: bool) -> Element {
    let detail = use_detail();
    if is_loading || !already_loaded {
        return rsx! {
            div { class: "discover-empty",
                i { class: "fa-solid fa-circle-notch fa-spin" }
                p { "Looking up related artists…" }
            }
        };
    }
    if related.is_empty() {
        return rsx! {
            div { class: "discover-empty",
                p { "No related artists turned up." }
            }
        };
    }
    rsx! {
        div { class: "related-grid",
            for ra in related.iter() {
                button {
                    class: "related-card",
                    key: "{ra.uri.0}",
                    onclick: {
                        let uri = ra.uri.clone();
                        move |_| detail.open_artist(uri.clone())
                    },
                    div { class: "related-avatar",
                        if let Some(img) = ra.image_url.as_ref() {
                            img { src: "{img}", alt: "", loading: "lazy", decoding: "async" }
                        } else {
                            span { class: "related-fallback",
                                "{ra.name.chars().next().unwrap_or('?')}"
                            }
                        }
                    }
                    div { class: "related-name", "{ra.name}" }
                    div { class: "related-sub", "via {ra.provider.label().to_lowercase()}" }
                }
            }
        }
    }
}
