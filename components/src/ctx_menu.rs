//! Right-click context menu surface.
//!
//! Sections (top→bottom):
//! 1. Header (title + artist · provider)
//! 2. Playback — Play next, Add to queue, Song Radio
//! 3. Download — FLAC via the hi-res provider (direct for the hi-res provider tracks, strict match
//!    for other providers; album entry on the hi-res provider tracks only)
//! 4. Save — Like / Unlike
//! 5. Navigate — Artist / Album

use std::sync::Arc;

use dioxus::prelude::*;
use hooks::{
    DiscoveryEngine, ProviderId, download_hires-provider_album, download_hires-provider_track,
    download_hires-provider_track_by_match, uri_has_detail_page, use_config, use_ctx_menu, use_detail,
    use_downloads, use_likes, use_local_library, use_hires-provider, use_queue,
};

#[component]
pub fn ContextMenu() -> Element {
    let ctx = use_ctx_menu();
    let queue = use_queue();
    let detail = use_detail();
    let likes = use_likes();
    let engine = use_context::<Arc<DiscoveryEngine>>();
    let qz = use_hires-provider();
    let config = use_config();
    let local = use_local_library();
    let downloads = use_downloads();
    let current = ctx.current.read().clone();

    let Some(state) = current else {
        return rsx! {};
    };

    let track = state.track.clone();
    let title = track.title.clone();
    let first_artist = track.artists.first().cloned();
    let artist_name = first_artist
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let artist_nav = first_artist
        .as_ref()
        .filter(|a| uri_has_detail_page(&a.uri.0))
        .map(|a| (a.uri.clone(), a.name.clone()));
    let album_nav = track
        .album
        .as_ref()
        .filter(|a| uri_has_detail_page(&a.uri.0))
        .map(|a| (a.uri.clone(), a.title.clone()));
    let provider = track.provider.label().to_string();
    let liked_now = likes.is_liked(&track.uri);
    let sub_text = if artist_name.is_empty() {
        provider.clone()
    } else {
        format!("{artist_name} · {provider}")
    };
    let like_label = if liked_now {
        "Remove from Liked"
    } else {
        "Save to Liked"
    };
    let like_icon_class = if liked_now {
        "ctx-icon accented fa-solid fa-heart"
    } else {
        "ctx-icon fa-regular fa-heart"
    };
    // Anchor-flip so the menu never overflows the window: near the right or
    // bottom edge, anchor the menu's right/bottom edge to the cursor instead
    // of its top-left. Thresholds are the worst-case menu size (320 wide,
    // ~460 tall) — flipping a little early just means the menu opens upward,
    // which is what native menus do too.
    let win = dioxus::desktop::window();
    let vp = win.inner_size().to_logical::<f64>(win.scale_factor());
    let h_anchor = if state.x > vp.width - 340.0 {
        format!("right: {:.0}px;", (vp.width - state.x).max(8.0))
    } else {
        format!("left: {:.0}px;", state.x)
    };
    let v_anchor = if state.y > vp.height - 480.0 {
        format!("bottom: {:.0}px;", (vp.height - state.y).max(8.0))
    } else {
        format!("top: {:.0}px;", state.y)
    };

    let artist_section = artist_nav.clone().map(|(uri, name)| {
        let label = format!("Go to {name}");
        rsx! {
            button {
                class: "ctx-item",
                onclick: move |_| {
                    detail.open_artist(uri.clone());
                    ctx.close();
                },
                i { class: "ctx-icon fa-solid fa-user" }
                div { class: "ctx-item-body",
                    div { class: "ctx-item-label", "{label}" }
                }
            }
        }
    });

    let album_section = album_nav.clone().map(|(uri, title_a)| {
        let label = format!("Go to {title_a}");
        rsx! {
            button {
                class: "ctx-item",
                onclick: move |_| {
                    detail.open_album(uri.clone());
                    ctx.close();
                },
                i { class: "ctx-icon fa-solid fa-compact-disc" }
                div { class: "ctx-item-body",
                    div { class: "ctx-item-label", "{label}" }
                }
            }
        }
    });

    let has_nav = artist_nav.is_some() || album_nav.is_some();

    // Download section — any streaming track can be grabbed as FLAC. A
    // the hi-res provider track downloads directly; other providers are matched on the hi-res provider
    // first (strict artist/title/duration match). The flows live in
    // `hooks::download_hires-provider_*` (shared with the album page): write hi-res
    // FLAC (MP3 fallback) into the library, then rescan so it shows up
    // under Library → Local. Local tracks are already on disk — no entry.
    let is_hires-provider = track.provider == ProviderId::the hi-res provider;
    let can_download = qz.is_connected() && track.provider != ProviderId::Local;
    let qz_album = track
        .album
        .as_ref()
        .filter(|_| is_hires-provider)
        .filter(|a| a.uri.0.starts_with("hires-provider:album:"))
        .map(|a| (a.uri.0.clone(), a.title.clone()));
    let library_root = config.read().library_root.clone();

    let download_track_section = can_download.then(|| {
        let qz = qz.clone();
        let local = local.clone();
        let root = library_root.clone();
        let track = track.clone();
        rsx! {
            button {
                class: "ctx-item",
                onclick: move |_| {
                    ctx.close();
                    if track.provider == ProviderId::the hi-res provider {
                        download_hires-provider_track(qz.clone(), local.clone(), downloads, root.clone(), track.uri.0.clone(), track.title.clone());
                    } else {
                        download_hires-provider_track_by_match(qz.clone(), local.clone(), downloads, root.clone(), track.clone());
                    }
                },
                i { class: "ctx-icon fa-solid fa-download" }
                div { class: "ctx-item-body",
                    div { class: "ctx-item-label", "Download (.flac)" }
                }
            }
        }
    });

    let download_album_section = qz_album.clone().map(|(uri, album_title)| {
        let qz = qz.clone();
        let local = local.clone();
        let root = library_root.clone();
        rsx! {
            button {
                class: "ctx-item",
                onclick: move |_| {
                    ctx.close();
                    download_hires-provider_album(qz.clone(), local.clone(), downloads, root.clone(), uri.clone(), album_title.clone());
                },
                i { class: "ctx-icon fa-solid fa-compact-disc" }
                div { class: "ctx-item-body",
                    div { class: "ctx-item-label", "Download album (.flac)" }
                }
            }
        }
    });

    let has_download = can_download;

    rsx! {
        button {
            class: "ctx-overlay",
            r#type: "button",
            onclick: move |_| ctx.close(),
            oncontextmenu: move |e: Event<MouseData>| {
                e.prevent_default();
                ctx.close();
            },
        }
        div {
            class: "ctx-menu",
            role: "menu",
            style: "{h_anchor} {v_anchor}",

            div { class: "ctx-header",
                div { class: "ctx-title", "{title}" }
                div { class: "ctx-sub", "{sub_text}" }
            }

            div { class: "ctx-group",
                button {
                    class: "ctx-item",
                    onclick: {
                        let queue = queue.clone();
                        let track = track.clone();
                        move |_| {
                            queue.play_next(track.clone());
                            ctx.close();
                        }
                    },
                    i { class: "ctx-icon fa-solid fa-arrow-right" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Play next" }
                    }
                }
                button {
                    class: "ctx-item",
                    onclick: {
                        let queue = queue.clone();
                        let track = track.clone();
                        move |_| {
                            queue.add_to_queue(track.clone());
                            ctx.close();
                        }
                    },
                    i { class: "ctx-icon fa-solid fa-list-ul" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Add to queue" }
                    }
                }
                button {
                    class: "ctx-item",
                    onclick: {
                        let queue = queue.clone();
                        let engine = engine.clone();
                        let track = track.clone();
                        move |_| {
                            queue.start_song_radio(track.clone(), engine.clone());
                            ctx.close();
                        }
                    },
                    i { class: "ctx-icon fa-solid fa-tower-broadcast" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Song Radio" }
                    }
                }
            }

            if has_download {
                div { class: "ctx-sep" }
                div { class: "ctx-group",
                    {download_track_section}
                    {download_album_section}
                }
            }

            div { class: "ctx-sep" }

            div { class: "ctx-group",
                button {
                    class: "ctx-item",
                    onclick: {
                        let track = track.clone();
                        move |_| {
                            likes.toggle(&track);
                            ctx.close();
                        }
                    },
                    i { class: "{like_icon_class}" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "{like_label}" }
                    }
                }
            }

            if has_nav {
                div { class: "ctx-sep" }
            }
            if has_nav {
                div { class: "ctx-group",
                    {artist_section}
                    {album_section}
                }
            }
        }
    }
}
