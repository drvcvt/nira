//! Library page — four tabs:
//!
//! - **Saved** — local likes stored on disk (cross-provider). Anything the
//!   user hearts in nira lands here, regardless of where it streams from.
//! - **Local** — the scanned `library_root` files, grouped into albums.
//! - **Playlists** — hand-curated cross-provider lists (JSON on disk).
//! - **Spotify Liked** — the Spotify-server-side liked songs list, pulled
//!   live via the API. Read-only mirror.

use std::sync::Arc;

use components::{Button, SearchBar};
use dioxus::prelude::*;
use hooks::{
    AlbumCtx, AlbumUri, LikedTrack, Playlist, PlaylistAlbum, Track, use_config, use_ctx_menu,
    use_detail, use_downloads, use_library, use_likes, use_local_library, use_playlists, use_queue,
    use_youtube,
};

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
    Local,
    Playlists,
    Spotify,
}

#[component]
pub fn Library() -> Element {
    let library = use_library();
    let likes = use_likes();
    let local = use_local_library();
    let playlists = use_playlists();
    let playlist_count = playlists.count();

    let mut tab = use_signal(|| LibTab::Saved);
    let active = *tab.read();

    let saved = likes.list();
    let local_context = use_memo(move || TrackContext::new(local.tracks.read().clone()));
    let local_count = local_context.read().len();
    let local_scanning = *local.is_scanning.read();
    let local_error = local.error.read().clone();
    // Wrap the (potentially ~900-track) Spotify-liked list in an Arc once
    // per render. Without this we'd hand a `Vec<Track>` to SpotifyLikedList,
    // which Dioxus diffs by `PartialEq` — a full O(N) walk — *and* the
    // component cloned the vec again internally. Arc-as-PartialEq is
    // pointer equality, so a no-op render is free.
    let spotify_context = use_memo(move || TrackContext::new(library.liked.read().clone()));
    let spotify_count = spotify_context.read().len();
    let is_loading = *library.is_loading.read();
    let lib_error = library.error.read().clone();
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
                    class: if active == LibTab::Local { "lib-tab active" } else { "lib-tab" },
                    onclick: move |_| tab.set(LibTab::Local),
                    i { class: "fa-solid fa-folder" }
                    " Local "
                    span { class: "lib-tab-count", "{local_count}" }
                }
                button {
                    class: if active == LibTab::Playlists { "lib-tab active" } else { "lib-tab" },
                    onclick: move |_| tab.set(LibTab::Playlists),
                    i { class: "fa-solid fa-list-ul" }
                    " Playlists "
                    span { class: "lib-tab-count", "{playlist_count}" }
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
                LibTab::Local => rsx! {
                    LocalList {
                        context: local_context.read().clone(),
                        is_scanning: local_scanning,
                        error: local_error.clone(),
                    }
                },
                LibTab::Playlists => rsx! { PlaylistsPane {} },
                LibTab::Spotify => rsx! {
                    SpotifyLikedList {
                        context: spotify_context.read().clone(),
                        is_loading,
                        lib_error: lib_error.clone(),
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
fn PlaylistsPane() -> Element {
    let playlists = use_playlists();
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let mut selected = use_signal(|| None::<String>);
    let mut new_name = use_signal(String::new);
    let mut importer_open = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    // In-place rename draft; None = showing the plain title.
    let mut editing_name = use_signal(|| None::<String>);
    // Leaving/entering a playlist resets the delete confirmation + rename.
    use_effect(move || {
        let _ = selected.read();
        confirm_delete.set(false);
        editing_name.set(None);
    });

    let items = playlists.list();
    let sel_pl: Option<Playlist> = selected
        .read()
        .as_ref()
        .and_then(|id| items.iter().find(|p| p.id == *id).cloned());

    // ── Detail view ─────────────────────────────────────────────
    if let Some(pl) = sel_pl {
        // Play/Shuffle feed the queue everything: loose tracks first, then
        // each album widget's tracks in display order.
        let all_tracks = pl.all_tracks();
        let total = all_tracks.len();
        let album_count = pl.albums.len();
        let count_label = if album_count > 0 {
            format!(
                "{total} {} · {album_count} {}",
                if total == 1 { "track" } else { "tracks" },
                if album_count == 1 { "album" } else { "albums" },
            )
        } else {
            format!("{total} {}", if total == 1 { "track" } else { "tracks" })
        };
        let context = TrackContext::new(pl.tracks.clone());
        let play_context = TrackContext::new(all_tracks);
        let pl_id = pl.id.clone();
        let confirm = *confirm_delete.read();
        let delete_label = if confirm { "Really delete?" } else { "Delete" };

        return rsx! {
            div { class: "lib-pl-head",
                button {
                    class: "sq-btn sq-btn-ghost sq-sm",
                    onclick: move |_| selected.set(None),
                    i { class: "fa-solid fa-arrow-left" }
                    " Playlists"
                }
                div { class: "lib-pl-copy",
                    if let Some(draft) = editing_name.read().clone() {
                        input {
                            class: "lib-pl-input lib-pl-rename",
                            value: "{draft}",
                            autofocus: true,
                            oninput: move |e: FormEvent| editing_name.set(Some(e.value())),
                            onkeydown: {
                                let pl_id = pl_id.clone();
                                move |e: KeyboardEvent| {
                                    if e.key() == Key::Enter {
                                        if let Some(name) = editing_name.peek().clone() {
                                            playlists.rename(&pl_id, &name);
                                        }
                                        editing_name.set(None);
                                    } else if e.key() == Key::Escape {
                                        editing_name.set(None);
                                    }
                                }
                            },
                        }
                    } else {
                        span { class: "lib-pl-title",
                            "{pl.name}"
                            button {
                                class: "lib-pl-edit",
                                title: "Rename playlist",
                                onclick: {
                                    let name = pl.name.clone();
                                    move |_| editing_name.set(Some(name.clone()))
                                },
                                i { class: "fa-solid fa-pen" }
                            }
                        }
                    }
                    span { class: "hint", "{count_label}" }
                }
                div { class: "lib-local-actions",
                    button {
                        class: "sq-btn sq-btn-ghost sq-sm",
                        disabled: total == 0,
                        onclick: {
                            let queue = queue.clone();
                            let play_context = play_context.clone();
                            move |_| queue.play_context(play_context.to_vec(), 0)
                        },
                        i { class: "fa-solid fa-play" }
                        " Play"
                    }
                    button {
                        class: "sq-btn sq-btn-ghost sq-sm",
                        disabled: total < 2,
                        onclick: {
                            let queue = queue.clone();
                            let play_context = play_context.clone();
                            move |_| queue.shuffle_all(play_context.to_vec())
                        },
                        i { class: "fa-solid fa-shuffle" }
                        " Shuffle"
                    }
                    button {
                        class: if confirm { "sq-btn sq-btn-ghost sq-sm active" } else { "sq-btn sq-btn-ghost sq-sm" },
                        onclick: {
                            let pl_id = pl_id.clone();
                            move |_| {
                                if *confirm_delete.peek() {
                                    playlists.delete(&pl_id);
                                    selected.set(None);
                                } else {
                                    confirm_delete.set(true);
                                }
                            }
                        },
                        i { class: "fa-solid fa-trash-can" }
                        " {delete_label}"
                    }
                }
            }
            if pl.is_empty() {
                div { class: "discover-empty",
                    div { class: "discover-empty-glyph",
                        i { class: "fa-solid fa-list-ul" }
                    }
                    p { "This playlist is empty." }
                    p { class: "hint", "Right-click any track or album → \"Add to playlist\" → {pl.name}." }
                }
            }
            if !pl.tracks.is_empty() {
                ul { class: "track-list",
                    for (idx, track) in pl.tracks.iter().enumerate() {
                        {
                            let track = track.clone();
                            let row_context = context.clone();
                            let pl_id = pl_id.clone();
                            let uri = track.uri.clone();
                            let track_total = pl.tracks.len();
                            let up = (idx > 0).then(|| {
                                let pl_id = pl_id.clone();
                                EventHandler::new(move |_| playlists.move_track(&pl_id, idx, -1))
                            });
                            let down = (idx + 1 < track_total).then(|| {
                                let pl_id = pl_id.clone();
                                EventHandler::new(move |_| playlists.move_track(&pl_id, idx, 1))
                            });
                            rsx! {
                                TrackRow {
                                    key: "{track.uri.0}",
                                    track: track.clone(),
                                    saved_at: None,
                                    show_unlike: true,
                                    remove_title: Some("Remove from playlist".to_string()),
                                    context: row_context,
                                    index: idx,
                                    on_move_up: up,
                                    on_move_down: down,
                                    on_unlike: move |_| playlists.remove_track(&pl_id, &uri),
                                }
                            }
                        }
                    }
                }
            }
            for (a_idx, album) in pl.albums.iter().enumerate() {
                PlaylistAlbumWidget {
                    key: "{album.uri}",
                    playlist_id: pl_id.clone(),
                    album: album.clone(),
                    index: a_idx,
                    total: pl.albums.len(),
                }
            }
        };
    }

    // ── Overview ────────────────────────────────────────────────
    rsx! {
        div { class: "lib-pl-create",
            input {
                class: "lib-pl-input",
                placeholder: "New playlist…",
                value: "{new_name}",
                oninput: move |e: FormEvent| new_name.set(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter && !new_name.peek().trim().is_empty() {
                        playlists.create(&new_name.peek());
                        new_name.set(String::new());
                    }
                },
            }
            button {
                class: "sq-btn sq-btn-ghost sq-sm",
                // Never disabled: an empty name creates "New Playlist"
                // (create() handles the default), and a dead button with a
                // not-allowed cursor reads as a bug.
                onclick: move |_| {
                    playlists.create(&new_name.peek());
                    new_name.set(String::new());
                },
                i { class: "fa-solid fa-plus" }
                " Create"
            }
            button {
                class: "sq-btn sq-btn-ghost sq-sm",
                onclick: move |_| importer_open.set(true),
                i { class: "fa-solid fa-file-import" }
                " Import"
            }
        }
        crate::library_import::PlaylistImporter { open: importer_open }

        if items.is_empty() {
            div { class: "discover-empty",
                div { class: "discover-empty-glyph",
                    i { class: "fa-solid fa-list-ul" }
                }
                p { "No playlists yet." }
                p { class: "hint",
                    "Create one above, import from Spotify, SoundCloud, or YouTube, or right-click any track and pick \"Add to playlist\"."
                }
            }
        } else {
            div { class: "album-grid local-album-grid",
                for pl in items.iter() {
                    {
                        let id = pl.id.clone();
                        let name = pl.name.clone();
                        let context_id = id.clone();
                        let context_name = name.clone();
                        let import_source = pl.import_source().map(str::to_owned);
                        let count = pl.tracks.len() + pl.albums.iter().map(|a| a.tracks.len()).sum::<usize>();
                        let cover = pl
                            .tracks
                            .iter()
                            .find_map(|t| t.cover_url.clone())
                            .or_else(|| pl.albums.iter().find_map(|a| a.cover_url.clone()));
                        let sub = if pl.albums.is_empty() {
                            format!("{count} {}", if count == 1 { "track" } else { "tracks" })
                        } else {
                            format!(
                                "{count} {} · {} {}",
                                if count == 1 { "track" } else { "tracks" },
                                pl.albums.len(),
                                if pl.albums.len() == 1 { "album" } else { "albums" },
                            )
                        };
                        rsx! {
                            button {
                                key: "{id}",
                                class: "album-card",
                                r#type: "button",
                                title: "{name}",
                                onclick: move |_| selected.set(Some(id.clone())),
                                oncontextmenu: move |e: Event<MouseData>| {
                                    let Some(source) = import_source.clone() else {
                                        return;
                                    };
                                    e.prevent_default();
                                    let pos = e.data.client_coordinates();
                                    ctx.open_playlist(
                                        pos.x,
                                        pos.y,
                                        context_id.clone(),
                                        context_name.clone(),
                                        source,
                                    );
                                },
                                div { class: "album-cover",
                                    if let Some(src) = cover.as_ref() {
                                        img { src: "{src}", alt: "", loading: "lazy", decoding: "async" }
                                    } else {
                                        span { class: "album-cover-fallback",
                                            i { class: "fa-solid fa-list-ul" }
                                        }
                                    }
                                }
                                div { class: "album-meta",
                                    span { class: "album-title", "{name}" }
                                    span { class: "album-sub", "{sub}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One embedded album inside a playlist — cover + meta header, expandable
/// track list, play/remove actions. Songs added normally stay loose rows;
/// this is the "whole album as a widget" path.
#[component]
fn PlaylistAlbumWidget(
    playlist_id: String,
    album: PlaylistAlbum,
    index: usize,
    total: usize,
) -> Element {
    let playlists = use_playlists();
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let mut expanded = use_signal(|| false);
    let is_open = *expanded.read();

    let count = album.tracks.len();
    let sub = format!(
        "{} · {} {}",
        album.artist,
        count,
        if count == 1 { "track" } else { "tracks" }
    );
    let cover = album.cover_url.clone().unwrap_or_default();
    let context = TrackContext::new(album.tracks.clone());
    let chevron = if is_open {
        "fa-solid fa-chevron-down"
    } else {
        "fa-solid fa-chevron-right"
    };

    rsx! {
        section { class: "pl-album",
            header { class: "pl-album-head",
                button {
                    class: "pl-album-main",
                    r#type: "button",
                    title: "{album.title} — {album.artist}",
                    onclick: move |_| {
                        let now = *expanded.peek();
                        expanded.set(!now);
                    },
                    oncontextmenu: {
                        let album = album.clone();
                        move |e: Event<MouseData>| {
                            e.prevent_default();
                            let pos = e.data.client_coordinates();
                            ctx.open_album(
                                pos.x,
                                pos.y,
                                AlbumCtx {
                                    uri: album.uri.clone(),
                                    title: album.title.clone(),
                                    artist: album.artist.clone(),
                                    cover_url: album.cover_url.clone(),
                                    tracks: album.tracks.clone(),
                                },
                            );
                        }
                    },
                    div { class: "pl-album-cover",
                        if !cover.is_empty() {
                            img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
                        } else {
                            i { class: "fa-solid fa-compact-disc" }
                        }
                    }
                    div { class: "pl-album-meta",
                        span { class: "pl-album-title", "{album.title}" }
                        span { class: "pl-album-sub", "{sub}" }
                    }
                    i { class: "pl-album-chevron {chevron}" }
                }
                div { class: "pl-album-actions",
                    button {
                        class: "sq-btn sq-btn-ghost sq-sm",
                        title: "Move up",
                        disabled: index == 0,
                        onclick: {
                            let playlist_id = playlist_id.clone();
                            move |_| playlists.move_album(&playlist_id, index, -1)
                        },
                        i { class: "fa-solid fa-chevron-up" }
                    }
                    button {
                        class: "sq-btn sq-btn-ghost sq-sm",
                        title: "Move down",
                        disabled: index + 1 >= total,
                        onclick: {
                            let playlist_id = playlist_id.clone();
                            move |_| playlists.move_album(&playlist_id, index, 1)
                        },
                        i { class: "fa-solid fa-chevron-down" }
                    }
                    button {
                        class: "sq-btn sq-btn-ghost sq-sm",
                        title: "Play this album",
                        disabled: count == 0,
                        onclick: {
                            let queue = queue.clone();
                            let context = context.clone();
                            move |_| queue.play_context(context.to_vec(), 0)
                        },
                        i { class: "fa-solid fa-play" }
                    }
                    button {
                        class: "sq-btn sq-btn-ghost sq-sm",
                        title: "Remove album from playlist",
                        onclick: {
                            let playlist_id = playlist_id.clone();
                            let uri = album.uri.clone();
                            move |_| playlists.remove_album(&playlist_id, &uri)
                        },
                        i { class: "fa-solid fa-xmark" }
                    }
                }
            }
            if is_open {
                ul { class: "track-list",
                    for (idx, track) in album.tracks.iter().enumerate() {
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
            }
        }
    }
}

/// How many album cards a "page" of the local grid shows before "Show more".
const LOCAL_ALBUM_PAGE: usize = 24;

#[component]
fn YouTubeDownloader(mut open: Signal<bool>) -> Element {
    let youtube = use_youtube();
    let downloads = use_downloads();
    let local = use_local_library();
    let config = use_config();
    let mut url = use_signal(String::new);
    let is_open = *open.read();
    let busy = *youtube.busy.read();
    let failed = *youtube.failed.read();
    let preview = youtube.preview.read().clone();
    let status = youtube.status.read().clone();
    let has_url = !url.read().trim().is_empty();
    let overlay_class = if is_open {
        "yt-downloader open"
    } else {
        "yt-downloader"
    };

    use_effect(move || {
        components::overlay_focus(*open.read(), ".yt-downloader.open .searchbar-input");
    });

    rsx! {
        div {
            class: "{overlay_class}",
            onkeydown: move |event: Event<KeyboardData>| {
                if event.key() == Key::Escape {
                    event.prevent_default();
                    open.set(false);
                }
            },
            button {
                class: "yt-downloader-backdrop",
                r#type: "button",
                tabindex: "-1",
                "aria-hidden": "true",
                onclick: move |_| open.set(false),
            }
            section {
                class: "yt-downloader-panel",
                role: "dialog",
                "aria-modal": "true",
                "aria-labelledby": "yt-downloader-title",
                header { class: "yt-downloader-head",
                    div {
                        h2 { id: "yt-downloader-title", "YouTube downloader" }
                        p { "Paste a song or video. Nira previews it before saving an MP3." }
                    }
                    button {
                        class: "yt-downloader-close",
                        title: "Close",
                        "aria-label": "Close YouTube downloader",
                        onclick: move |_| open.set(false),
                        i { class: "fa-solid fa-xmark" }
                    }
                }

                div { class: "searchbar-row yt-downloader-form",
                    SearchBar {
                        key: "youtube-downloader-input-{is_open}",
                        icon: Some("fa-brands fa-youtube".to_string()),
                        value: url.read().clone(),
                        placeholder: "https://youtube.com/watch?v=…".to_string(),
                        autofocus: is_open,
                        on_input: move |value: String| url.set(value),
                        on_submit: move |_| {
                            let value = url.peek().clone();
                            if !*youtube.busy.peek() && !value.trim().is_empty() {
                                youtube.preview(value);
                            }
                        },
                    }
                    button {
                        class: "sq-btn sq-btn-ghost sq-md",
                        disabled: busy || !has_url,
                        onclick: move |_| youtube.preview(url.peek().clone()),
                        if busy {
                            i { class: "fa-solid fa-circle-notch fa-spin" }
                            " Working"
                        } else {
                            i { class: "fa-solid fa-arrow-right" }
                            " Preview"
                        }
                    }
                }

                if let Some(item) = preview.as_ref() {
                    div { class: "yt-preview",
                        div { class: "yt-preview-cover",
                            if let Some(thumbnail) = item.thumbnail.as_ref() {
                                img {
                                    src: "{thumbnail}",
                                    alt: "",
                                    loading: "lazy",
                                    decoding: "async",
                                }
                            } else {
                                i { class: "fa-solid fa-music" }
                            }
                        }
                        div { class: "yt-preview-meta",
                            strong { "{item.title}" }
                            span {
                                "{item.uploader}"
                                if let Some(duration) = item.duration {
                                    " · {hooks::fmt_time(duration)}"
                                }
                            }
                        }
                        Button {
                            label: if busy { "Working".to_string() } else { "Download MP3".to_string() },
                            icon: Some(if busy {
                                "fa-solid fa-circle-notch fa-spin".to_string()
                            } else {
                                "fa-solid fa-download".to_string()
                            }),
                            disabled: busy,
                            on_click: move |_| {
                                youtube.download(
                                    local,
                                    downloads,
                                    config.peek().library_root.clone(),
                                )
                            },
                        }
                    }
                } else if status.is_none() {
                    div { class: "yt-downloader-idle",
                        i { class: "fa-solid fa-music" }
                        p { "Preview first. Downloads continue in the background if you close this." }
                    }
                }

                if let Some(message) = status.as_ref() {
                    p {
                        class: "yt-import-status",
                        role: "status",
                        "aria-live": "polite",
                        if busy {
                            i { class: "fa-solid fa-circle-notch fa-spin" }
                        } else if failed {
                            i { class: "fa-solid fa-circle-exclamation" }
                        } else {
                            i { class: "fa-solid fa-check" }
                        }
                        "{message}"
                    }
                }
            }
        }
    }
}

#[component]
fn LocalList(context: TrackContext, is_scanning: bool, error: Option<String>) -> Element {
    let local = use_local_library();
    let queue = use_queue();
    let total = context.len();
    let mut visible_albums = use_signal(|| LOCAL_ALBUM_PAGE);
    let mut lossless_only = use_signal(|| false);
    let mut youtube_open = use_signal(|| false);
    let only_lossless = *lossless_only.read();
    let lossy_total = context.iter().filter(|t| !is_lossless(&t.uri.0)).count();
    let size_on_disk = *local.total_bytes.read();

    let (albums, singles) = local_albums(&context, only_lossless);
    let album_total = albums.len();
    let shown = (*visible_albums.read()).min(album_total);

    rsx! {
        YouTubeDownloader { open: youtube_open }

        div { class: "lib-local-head",
            p { class: "hint",
                "Albums from your local music folder. Click one to open it."
            }
            div { class: "lib-local-actions",
                button {
                    class: "sq-btn sq-btn-ghost sq-sm",
                    title: "Download a YouTube song with yt-dlp",
                    onclick: move |_| youtube_open.set(true),
                    i { class: "fa-brands fa-youtube" }
                    " YouTube download"
                }
                button {
                    class: "sq-btn sq-btn-ghost sq-sm",
                    title: "Shuffle the whole local library",
                    disabled: total == 0,
                    onclick: {
                        let context = context.clone();
                        let queue = queue.clone();
                        move |_| {
                            let tracks: Vec<Track> = context
                                .iter()
                                .filter(|t| !*lossless_only.peek() || is_lossless(&t.uri.0))
                                .cloned()
                                .collect();
                            queue.shuffle_all(tracks);
                        }
                    },
                    i { class: "fa-solid fa-shuffle" }
                    " Shuffle all"
                }
                if lossy_total > 0 {
                    button {
                        class: if only_lossless { "sq-btn sq-btn-ghost sq-sm active" } else { "sq-btn sq-btn-ghost sq-sm" },
                        title: "Hide MP3 / lossy files",
                        onclick: move |_| lossless_only.toggle(),
                        i { class: "fa-solid fa-gem" }
                        " Lossless only"
                    }
                }
                button {
                    class: "sq-btn sq-btn-ghost sq-sm",
                    disabled: is_scanning,
                    onclick: move |_| local.rescan(),
                    if is_scanning {
                        i { class: "fa-solid fa-circle-notch fa-spin" }
                        " Scanning…"
                    } else {
                        i { class: "fa-solid fa-rotate" }
                        " Rescan"
                    }
                }
            }
        }

        if let Some(err) = error.as_ref() {
            div { class: "search-error", "{err}" }
        }

        if total == 0 && !is_scanning {
            div { class: "discover-empty",
                div { class: "discover-empty-glyph",
                    i { class: "fa-solid fa-folder-open" }
                }
                p { "No local tracks yet." }
                p { class: "hint",
                    "Set your music folder in Settings → Library, then Rescan. FLAC, MP3, M4A, OGG and WAV are supported."
                }
            }
        }

        if total > 0 {
            p { class: "hint",
                {
                    let size = human_bytes(size_on_disk);
                    let albums_word = if album_total == 1 { "album" } else { "albums" };
                    if only_lossless {
                        format!("{album_total} {albums_word} · {total} tracks · {lossy_total} lossy hidden · {size} on disk")
                    } else {
                        format!("{album_total} {albums_word} · {total} tracks · {size} on disk")
                    }
                }
            }
            div { class: "album-grid local-album-grid",
                for album in albums.iter().take(shown) {
                    LocalAlbumCard {
                        key: "{album.uri}",
                        album_uri: album.uri.clone(),
                        title: album.title.clone(),
                        artist: album.artist.clone(),
                        cover: album.cover.clone(),
                        count: album.count,
                        lossless: album.lossless,
                    }
                }
            }
            if shown < album_total {
                button {
                    class: "sq-btn sq-btn-ghost sq-sm library-more-btn",
                    onclick: move |_| visible_albums.set((shown + LOCAL_ALBUM_PAGE).min(album_total)),
                    "Show more"
                }
            }
            if !singles.is_empty() {
                section { class: "lib-album",
                    header { class: "lib-album-head",
                        span { class: "lib-album-title", "Singles" }
                        span { class: "lib-album-meta",
                            {format!(
                                "{} {}",
                                singles.len(),
                                if singles.len() == 1 { "track" } else { "tracks" }
                            )}
                        }
                    }
                    ul { class: "track-list",
                        for (idx, track) in singles.iter() {
                            {
                                let track = track.clone();
                                let row_context = context.clone();
                                let index = *idx;
                                let badge = local_format_label(&track.uri.0);
                                rsx! {
                                    TrackRow {
                                        key: "{track.uri.0}",
                                        track: track.clone(),
                                        saved_at: None,
                                        show_unlike: false,
                                        context: row_context,
                                        index,
                                        quality_badge: badge,
                                        on_unlike: move |_| {},
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn LocalAlbumCard(
    album_uri: String,
    title: String,
    artist: String,
    cover: Option<String>,
    count: usize,
    lossless: bool,
) -> Element {
    let detail = use_detail();
    let ctx = use_ctx_menu();
    let local = use_local_library();
    let sub = format!(
        "{artist} · {count} {}{}",
        if count == 1 { "track" } else { "tracks" },
        if lossless { " · lossless" } else { "" },
    );
    let uri_for_click = album_uri.clone();
    rsx! {
        button {
            class: "album-card",
            r#type: "button",
            title: "{title} — {artist}",
            onclick: move |_| detail.open_album(AlbumUri(uri_for_click.clone())),
            oncontextmenu: {
                let album_uri = album_uri.clone();
                let title = title.clone();
                let artist = artist.clone();
                let cover = cover.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    // The scanner sorts by album, so this filter IS the
                    // album's track list in disc/track order.
                    let tracks: Vec<Track> = local
                        .tracks
                        .peek()
                        .iter()
                        .filter(|t| t.album.as_ref().is_some_and(|a| a.uri.0 == album_uri))
                        .cloned()
                        .collect();
                    ctx.open_album(
                        pos.x,
                        pos.y,
                        AlbumCtx {
                            uri: album_uri.clone(),
                            title: title.clone(),
                            artist: artist.clone(),
                            cover_url: cover.clone(),
                            tracks,
                        },
                    );
                }
            },
            div { class: "album-cover",
                if let Some(src) = cover.as_ref() {
                    img { src: "{src}", alt: "", loading: "lazy", decoding: "async" }
                } else {
                    span { class: "album-cover-fallback",
                        i { class: "fa-solid fa-compact-disc" }
                    }
                }
            }
            div { class: "album-meta",
                span { class: "album-title", "{title}" }
                span { class: "album-sub", "{sub}" }
            }
        }
    }
}

struct LocalAlbum {
    uri: String,
    title: String,
    artist: String,
    cover: Option<String>,
    count: usize,
    /// Every track in the album is lossless (drives the card's format hint).
    lossless: bool,
}

/// Split the scanned library into album cards and album-less singles. The
/// scanner sorts artist → album → disc → track, so consecutive tracks with
/// the same album URI ARE the album. Singles keep their global context
/// index so a row click queues the whole library from that spot.
fn local_albums(
    context: &TrackContext,
    only_lossless: bool,
) -> (Vec<LocalAlbum>, Vec<(usize, Track)>) {
    let mut albums: Vec<LocalAlbum> = Vec::new();
    let mut singles: Vec<(usize, Track)> = Vec::new();
    for (idx, track) in context.iter().enumerate() {
        if only_lossless && !is_lossless(&track.uri.0) {
            continue;
        }
        let Some(album_ref) = track.album.as_ref() else {
            singles.push((idx, track.clone()));
            continue;
        };
        match albums.last_mut() {
            Some(a) if a.uri == album_ref.uri.0 => {
                a.count += 1;
                a.lossless &= is_lossless(&track.uri.0);
                if a.cover.is_none() {
                    a.cover = track.cover_url.clone();
                }
            }
            _ => albums.push(LocalAlbum {
                uri: album_ref.uri.0.clone(),
                title: album_ref.title.clone(),
                artist: track
                    .artists
                    .first()
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| "Unknown Artist".to_string()),
                cover: track.cover_url.clone(),
                count: 1,
                lossless: is_lossless(&track.uri.0),
            }),
        }
    }
    (albums, singles)
}

/// Human-readable byte size (binary units, one decimal from MB up).
fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let b = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// Extension of a `local:track:<path>` URI, lowercased.
fn local_ext(uri: &str) -> Option<String> {
    uri.rsplit('.').next().map(|e| e.to_ascii_lowercase())
}

/// Whether a local track is lossless, judged by its file extension. We only
/// ever write `.flac`/`.mp3`, and the user's own files carry real
/// extensions — so the extension is a reliable lossless/lossy split without
/// re-reading every file's codec.
fn is_lossless(uri: &str) -> bool {
    matches!(
        local_ext(uri).as_deref(),
        Some("flac" | "wav" | "aif" | "aiff" | "alac")
    )
}

/// Short format badge for a local row (FLAC / MP3 / …). None for non-local.
fn local_format_label(uri: &str) -> Option<String> {
    if !uri.starts_with("local:track:") {
        return None;
    }
    Some(match local_ext(uri).as_deref() {
        Some("flac") => "FLAC",
        Some("wav") => "WAV",
        Some("aif" | "aiff") => "AIFF",
        Some("alac" | "m4a") => "ALAC",
        Some("mp3") => "MP3",
        Some("aac" | "mp4") => "AAC",
        Some("ogg" | "oga") => "OGG",
        Some("opus") => "OPUS",
        _ => "AUDIO",
    }
    .to_string())
}

#[component]
fn SpotifyLikedList(
    context: TrackContext,
    is_loading: bool,
    lib_error: Option<String>,
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
            if let Some(err) = lib_error.as_ref() {
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
    #[props(default)] quality_badge: Option<String>,
    /// Tooltip for the trailing remove button; defaults to the Saved tab's
    /// "Remove from Liked".
    #[props(default)] remove_title: Option<String>,
    /// Reorder handles (playlist rows). None on other tabs = no buttons.
    #[props(default)] on_move_up: Option<EventHandler<()>>,
    #[props(default)] on_move_down: Option<EventHandler<()>>,
    on_unlike: EventHandler<()>,
) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let duration = format_duration(track.duration);
    let cover = track.cover_url.clone().unwrap_or_default();
    let badge_class = provider_badge_class(track.provider);
    // Local rows show their audio format (FLAC/MP3/…) instead of the generic
    // "L" provider badge; lossy formats get a dimmed variant to spot at a glance.
    let lossy = quality_badge
        .as_deref()
        .is_some_and(|b| !matches!(b, "FLAC" | "WAV" | "AIFF" | "ALAC"));
    let saved_str = saved_at.map(fmt_relative).unwrap_or_default();
    let play_context = context.clone();
    let ctx_track = track.clone();

    let key_context = context.clone();
    let key_queue = queue.clone();
    rsx! {
        li {
            class: "track-row",
            tabindex: "0",
            role: "button",
            onclick: move |_| queue.play_context(play_context.to_vec(), index),
            onkeydown: move |e: KeyboardEvent| {
                let key = e.key();
                let is_space = key.to_string() == " ";
                if key == Key::Enter || is_space {
                    e.prevent_default();
                    if is_space {
                        e.stop_propagation();
                    }
                    key_queue.play_context(key_context.to_vec(), index);
                }
            },
            oncontextmenu: move |e: Event<MouseData>| open_track_context(ctx, e, ctx_track.clone()),
            div { class: "track-cover",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
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
            if let Some(fmt) = quality_badge.as_ref() {
                div {
                    class: if lossy { "track-badge track-badge-lossy" } else { "track-badge" },
                    "{fmt}"
                }
            } else {
                div { class: "{badge_class}", "{track.provider.badge()}" }
            }
            if on_move_up.is_some() || on_move_down.is_some() {
                div { class: "track-row-reorder",
                    button {
                        class: "track-row-move",
                        title: "Move up",
                        disabled: on_move_up.is_none(),
                        onkeydown: |e: KeyboardEvent| e.stop_propagation(),
                        onclick: move |e: MouseEvent| {
                            e.stop_propagation();
                            if let Some(h) = on_move_up {
                                h.call(());
                            }
                        },
                        i { class: "fa-solid fa-chevron-up" }
                    }
                    button {
                        class: "track-row-move",
                        title: "Move down",
                        disabled: on_move_down.is_none(),
                        onkeydown: |e: KeyboardEvent| e.stop_propagation(),
                        onclick: move |e: MouseEvent| {
                            e.stop_propagation();
                            if let Some(h) = on_move_down {
                                h.call(());
                            }
                        },
                        i { class: "fa-solid fa-chevron-down" }
                    }
                }
            }
            if show_unlike {
                button {
                    class: "track-row-unlike",
                    title: remove_title.as_deref().unwrap_or("Remove from Liked"),
                    onkeydown: |e: KeyboardEvent| e.stop_propagation(),
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

#[cfg(test)]
mod tests {
    use super::{human_bytes, is_lossless, local_format_label};

    #[test]
    fn human_bytes_picks_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn lossless_split_by_extension() {
        assert!(is_lossless("local:track:/m/a.flac"));
        assert!(!is_lossless("local:track:/m/a.mp3"));
        assert_eq!(local_format_label("local:track:/m/a.flac").as_deref(), Some("FLAC"));
        assert_eq!(local_format_label("local:track:/m/a.mp3").as_deref(), Some("MP3"));
        assert_eq!(local_format_label("spotify:track:123"), None);
    }
}
