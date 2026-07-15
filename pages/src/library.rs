//! Library page — two tabs:
//!
//! - **Saved** — local likes stored on disk (cross-provider). Anything the
//!   user hearts in nira lands here, regardless of where it streams from.
//! - **Spotify Liked** — the Spotify-server-side liked songs list, pulled
//!   live via the API. Read-only mirror.

use std::sync::Arc;

use dioxus::prelude::*;
use hooks::{
    AlbumUri, LikedTrack, Track, use_ctx_menu, use_detail, use_library, use_likes,
    use_local_library, use_queue,
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
    Spotify,
}

#[component]
pub fn Library() -> Element {
    let library = use_library();
    let likes = use_likes();
    let local = use_local_library();

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

/// How many album cards a "page" of the local grid shows before "Show more".
const LOCAL_ALBUM_PAGE: usize = 24;

#[component]
fn LocalList(context: TrackContext, is_scanning: bool, error: Option<String>) -> Element {
    let local = use_local_library();
    let queue = use_queue();
    let total = context.len();
    let mut visible_albums = use_signal(|| LOCAL_ALBUM_PAGE);
    let mut lossless_only = use_signal(|| false);
    let only_lossless = *lossless_only.read();
    let lossy_total = context.iter().filter(|t| !is_lossless(&t.uri.0)).count();
    let size_on_disk = *local.total_bytes.read();

    let (albums, singles) = local_albums(&context, only_lossless);
    let album_total = albums.len();
    let shown = (*visible_albums.read()).min(album_total);

    rsx! {
        div { class: "lib-local-head",
            p { class: "hint",
                "Albums from your local music folder. Click one to open it."
            }
            div { class: "lib-local-actions",
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
    let sub = format!(
        "{artist} · {count} {}{}",
        if count == 1 { "track" } else { "tracks" },
        if lossless { " · lossless" } else { "" },
    );
    rsx! {
        button {
            class: "album-card",
            r#type: "button",
            title: "{title} — {artist}",
            onclick: move |_| detail.open_album(AlbumUri(album_uri.clone())),
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
/// ever write `.flac`/`.mp3` from the hi-res provider, and the user's own files carry real
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
                if e.key() == Key::Enter {
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
        assert_eq!(local_format_label("hires-provider:track:123"), None);
    }
}
