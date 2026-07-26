//! Right-click context menu surface.
//!
//! Sections (top→bottom):
//! 1. Header (title + artist · provider)
//! 2. Playback — Play next, Add to queue, Song Radio
//! 3. Download — FLAC via the hi-res provider (direct for the hi-res provider tracks, strict match
//!    for other providers; album entry on the hi-res provider tracks only)
//! 4. Save — Like / Unlike, Add to playlist (expands in place)
//! 5. Navigate — Artist / Album
//! 6. History — Remove from history (only when opened from a history card)

use std::sync::Arc;

use dioxus::prelude::*;
use hooks::{
    AlbumCtx, AlbumUri, CtxTarget, DiscoveryEngine, PlaylistAlbum, ProviderId,
    download_hires-provider_album, download_hires-provider_track, download_hires-provider_track_by_match,
    uri_has_detail_page, use_config, use_ctx_menu, use_detail, use_downloads, use_history,
    use_likes, use_local_library, use_playlists, use_hires-provider, use_queue,
};

/// Anchor-flip so the menu never overflows the window: near the right or
/// bottom edge, anchor the menu's right/bottom edge to the cursor instead
/// of its top-left. Thresholds are the worst-case menu size (320 wide,
/// ~460 tall) — flipping a little early just means the menu opens upward,
/// which is what native menus do too.
fn anchor_style(x: f64, y: f64) -> (String, String) {
    let win = dioxus::desktop::window();
    let vp = win.inner_size().to_logical::<f64>(win.scale_factor());
    let h = if x > vp.width - 340.0 {
        format!("right: {:.0}px;", (vp.width - x).max(8.0))
    } else {
        format!("left: {:.0}px;", x)
    };
    let v = if y > vp.height - 480.0 {
        format!("bottom: {:.0}px;", (vp.height - y).max(8.0))
    } else {
        format!("top: {:.0}px;", y)
    };
    (h, v)
}

fn track_download_label(provider: ProviderId) -> &'static str {
    if provider == ProviderId::the hi-res provider {
        "Download (.flac)"
    } else {
        "Find on the hi-res provider & download"
    }
}

/// Move the roving keyboard focus over the menu's enabled items by `delta`
/// (wraps at both ends). DOM focus is the selection cursor — the shell's JS
/// hotkey listener stands down while a `.ctx-menu` is open, so arrow keys
/// land here instead of on the volume binds.
fn ctx_focus_move(delta: i32) {
    document::eval(&format!(
        "(function() {{\
            var m = document.querySelector('.ctx-menu');\
            if (!m) return;\
            var items = Array.prototype.slice.call(m.querySelectorAll('.ctx-item:not(:disabled)'));\
            if (!items.length) return;\
            var i = items.indexOf(document.activeElement);\
            var n = i < 0 ? ({delta} > 0 ? 0 : items.length - 1)\
                          : (i + {delta} + items.length) % items.length;\
            items[n].focus();\
        }})();"
    ));
}

/// Shared arrow-key handler for both menu variants.
fn ctx_menu_keynav(e: Event<KeyboardData>) {
    let delta = match e.key() {
        Key::ArrowDown => 1,
        Key::ArrowUp => -1,
        _ => return,
    };
    e.prevent_default();
    ctx_focus_move(delta);
}

#[component]
pub fn ContextMenu() -> Element {
    let ctx = use_ctx_menu();
    let queue = use_queue();
    let detail = use_detail();
    let likes = use_likes();
    let playlists = use_playlists();
    let history = use_history();
    let engine = use_context::<Arc<DiscoveryEngine>>();
    let qz = use_hires-provider();
    let config = use_config();
    let local = use_local_library();
    let downloads = use_downloads();
    // Inline "Add to playlist" expansion state — reset whenever the menu
    // opens/closes so every fresh right-click starts collapsed.
    let mut show_playlists = use_signal(|| false);
    use_effect(move || {
        let open = ctx.current.read().is_some();
        show_playlists.set(false);
        // Seed the roving keyboard focus on the first item once the menu
        // exists in the DOM, and hand focus back to whatever had it when the
        // menu closes — otherwise Escape drops focus on <body> and the next
        // Tab restarts from the sidebar instead of the row you right-clicked.
        crate::overlay_focus(open, ".ctx-menu .ctx-item");
    });
    let current = ctx.current.read().clone();

    let Some(state) = current else {
        return rsx! {};
    };

    // Album target renders its own, smaller menu.
    if let CtxTarget::Album(album) = &state.target {
        return rsx! {
            AlbumCtxMenu {
                x: state.x,
                y: state.y,
                album: album.clone(),
                show_playlists,
            }
        };
    }
    let CtxTarget::Track(track) = state.target.clone() else {
        return rsx! {};
    };
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
    let (h_anchor, v_anchor) = anchor_style(state.x, state.y);

    let artist_section = artist_nav.clone().map(|(uri, name)| {
        let label = format!("Go to {name}");
        rsx! {
            button {
                class: "ctx-item",
                role: "menuitem",
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
                role: "menuitem",
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
    let download_label = track_download_label(track.provider);
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
                role: "menuitem",
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
                    div { class: "ctx-item-label", "{download_label}" }
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
                role: "menuitem",
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

    // History card origin → offer removing exactly that log row.
    let history_section = state.history_entry.clone().map(|entry| {
        let history = history.clone();
        rsx! {
            div { class: "ctx-sep" }
            div { class: "ctx-group",
                button {
                    class: "ctx-item",
                role: "menuitem",
                    onclick: move |_| {
                        history.remove(&entry);
                        ctx.close();
                    },
                    i { class: "ctx-icon fa-solid fa-clock-rotate-left" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Remove from history" }
                    }
                }
            }
        }
    });

    let pl_list = playlists.list();
    let pl_open = *show_playlists.read();
    let pl_chevron = if pl_open {
        "ctx-chevron fa-solid fa-chevron-down"
    } else {
        "ctx-chevron fa-solid fa-chevron-right"
    };

    rsx! {
        button {
            class: "ctx-overlay",
            r#type: "button",
            tabindex: "-1",
            "aria-hidden": "true",
            onclick: move |_| ctx.close(),
            oncontextmenu: move |e: Event<MouseData>| {
                e.prevent_default();
                ctx.close();
            },
        }
        div {
            class: "ctx-menu",
            role: "menu",
            "aria-label": "Track actions",
            onkeydown: ctx_menu_keynav,
            style: "{h_anchor} {v_anchor}",

            div { class: "ctx-header",
                div { class: "ctx-title", "{title}" }
                div { class: "ctx-sub", "{sub_text}" }
            }

            div { class: "ctx-group",
                button {
                    class: "ctx-item",
                role: "menuitem",
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
                role: "menuitem",
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
                role: "menuitem",
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
                role: "menuitem",
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
                button {
                    class: "ctx-item",
                role: "menuitem",
                    onclick: move |_| {
                        let open = *show_playlists.peek();
                        show_playlists.set(!open);
                    },
                    i { class: "ctx-icon fa-solid fa-list-ul" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Add to playlist" }
                    }
                    i { class: "{pl_chevron}" }
                }
                if pl_open {
                    PlaylistPicker {
                        contained: pl_list
                            .iter()
                            .filter(|p| p.tracks.iter().any(|t| t.uri == track.uri))
                            .map(|p| p.id.clone())
                            .collect::<Vec<_>>(),
                        on_pick: {
                            let track = track.clone();
                            move |(id, is_in): (String, bool)| {
                                if is_in {
                                    playlists.remove_track(&id, &track.uri);
                                } else {
                                    playlists.add_track(&id, &track);
                                }
                                ctx.close();
                            }
                        },
                        on_create: {
                            let track = track.clone();
                            move |name: String| {
                                let id = playlists.create(&name);
                                playlists.add_track(&id, &track);
                                ctx.close();
                            }
                        },
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

            {history_section}
        }
    }
}

/// Inline playlist chooser shared by the track and album menus. Rows call
/// `on_pick(id, already_contained)`; the trailing input row creates a new
/// playlist via `on_create(name)` — the caller seeds it with the item.
#[component]
fn PlaylistPicker(
    contained: Vec<String>,
    on_pick: EventHandler<(String, bool)>,
    on_create: EventHandler<String>,
) -> Element {
    let playlists = use_playlists();
    let mut name = use_signal(String::new);
    let items = playlists.list();

    rsx! {
        div { class: "ctx-sublist",
            for pl in items.iter() {
                {
                    let id = pl.id.clone();
                    let is_in = contained.contains(&id);
                    let pl_name = pl.name.clone();
                    let count = pl.tracks.len() + pl.albums.len();
                    let icon = if is_in {
                        "ctx-icon fa-solid fa-check"
                    } else {
                        "ctx-icon fa-regular fa-square-plus"
                    };
                    rsx! {
                        button {
                            key: "{id}",
                            class: "ctx-item ctx-subitem",
                            role: "menuitem",
                            onclick: move |_| on_pick.call((id.clone(), is_in)),
                            i { class: "{icon}" }
                            div { class: "ctx-item-body",
                                div { class: "ctx-item-label", "{pl_name}" }
                            }
                            span { class: "ctx-subcount", "{count}" }
                        }
                    }
                }
            }
            div { class: "ctx-newpl",
                input {
                    class: "ctx-newpl-input",
                    placeholder: "New playlist…",
                    value: "{name}",
                    onclick: move |e: Event<MouseData>| e.stop_propagation(),
                    oninput: move |e: FormEvent| name.set(e.value()),
                    onkeydown: move |e: Event<KeyboardData>| {
                        if e.key() == Key::Enter && !name.peek().trim().is_empty() {
                            on_create.call(name.peek().clone());
                        }
                    },
                }
                button {
                    class: "ctx-newpl-btn",
                    title: "Create playlist with this item",
                    onclick: move |_| {
                        if !name.peek().trim().is_empty() {
                            on_create.call(name.peek().clone());
                        }
                    },
                    i { class: "fa-solid fa-plus" }
                }
            }
        }
    }
}

/// The album right-click menu — album cards and the album-page banner open
/// it via `ctx.open_album`. Adding to a playlist embeds the album as a
/// widget (see `PlaylistAlbum`), not as loose rows.
#[component]
fn AlbumCtxMenu(x: f64, y: f64, album: AlbumCtx, show_playlists: Signal<bool>) -> Element {
    let ctx = use_ctx_menu();
    let queue = use_queue();
    let detail = use_detail();
    let playlists = use_playlists();

    let track_count = album.tracks.len();
    // Empty tracks = the opener is still resolving the album detail (cards
    // open instantly); show "…" and keep track-dependent entries disabled.
    let has_tracks = track_count > 0;
    let sub_text = if has_tracks {
        format!(
            "{} · {} {}",
            album.artist,
            track_count,
            if track_count == 1 { "track" } else { "tracks" }
        )
    } else {
        format!("{} · …", album.artist)
    };
    let (h_anchor, v_anchor) = anchor_style(x, y);
    let pl_open = *show_playlists.read();
    let pl_chevron = if pl_open {
        "ctx-chevron fa-solid fa-chevron-down"
    } else {
        "ctx-chevron fa-solid fa-chevron-right"
    };
    let contained: Vec<String> = playlists
        .list()
        .iter()
        .filter(|p| p.albums.iter().any(|a| a.uri == album.uri))
        .map(|p| p.id.clone())
        .collect();
    let mut show = show_playlists;

    rsx! {
        button {
            class: "ctx-overlay",
            r#type: "button",
            tabindex: "-1",
            "aria-hidden": "true",
            onclick: move |_| ctx.close(),
            oncontextmenu: move |e: Event<MouseData>| {
                e.prevent_default();
                ctx.close();
            },
        }
        div {
            class: "ctx-menu",
            role: "menu",
            "aria-label": "Album actions",
            onkeydown: ctx_menu_keynav,
            style: "{h_anchor} {v_anchor}",

            div { class: "ctx-header",
                div { class: "ctx-title", "{album.title}" }
                div { class: "ctx-sub", "{sub_text}" }
            }

            div { class: "ctx-group",
                button {
                    class: "ctx-item",
                role: "menuitem",
                    disabled: !has_tracks,
                    onclick: {
                        let queue = queue.clone();
                        let tracks = album.tracks.clone();
                        move |_| {
                            if !tracks.is_empty() {
                                queue.play_context(tracks.clone(), 0);
                            }
                            ctx.close();
                        }
                    },
                    i { class: "ctx-icon fa-solid fa-play" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Play album" }
                    }
                }
                button {
                    class: "ctx-item",
                role: "menuitem",
                    disabled: !has_tracks,
                    onclick: {
                        let queue = queue.clone();
                        let tracks = album.tracks.clone();
                        move |_| {
                            queue.add_all(tracks.clone());
                            ctx.close();
                        }
                    },
                    i { class: "ctx-icon fa-solid fa-list-ul" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Add to queue" }
                    }
                }
            }

            div { class: "ctx-sep" }

            div { class: "ctx-group",
                button {
                    class: "ctx-item",
                role: "menuitem",
                    disabled: !has_tracks,
                    onclick: move |_| {
                        let open = *show.peek();
                        show.set(!open);
                    },
                    i { class: "ctx-icon fa-solid fa-square-plus" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Add to playlist" }
                    }
                    i { class: "{pl_chevron}" }
                }
                if pl_open {
                    PlaylistPicker {
                        contained,
                        on_pick: {
                            let album = album.clone();
                            move |(id, is_in): (String, bool)| {
                                if is_in {
                                    playlists.remove_album(&id, &album.uri);
                                } else {
                                    playlists.add_album(&id, &PlaylistAlbum::from_ctx(&album));
                                }
                                ctx.close();
                            }
                        },
                        on_create: {
                            let album = album.clone();
                            move |name: String| {
                                let id = playlists.create(&name);
                                playlists.add_album(&id, &PlaylistAlbum::from_ctx(&album));
                                ctx.close();
                            }
                        },
                    }
                }
            }

            div { class: "ctx-sep" }

            div { class: "ctx-group",
                button {
                    class: "ctx-item",
                role: "menuitem",
                    onclick: {
                        let uri = album.uri.clone();
                        move |_| {
                            detail.open_album(AlbumUri(uri.clone()));
                            ctx.close();
                        }
                    },
                    i { class: "ctx-icon fa-solid fa-compact-disc" }
                    div { class: "ctx-item-body",
                        div { class: "ctx-item-label", "Go to album" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_provider_download_label_names_hires-provider_search() {
        assert_eq!(track_download_label(ProviderId::the hi-res provider), "Download (.flac)");
        assert_eq!(
            track_download_label(ProviderId::Spotify),
            "Find on the hi-res provider & download"
        );
        assert_eq!(
            track_download_label(ProviderId::SoundCloud),
            "Find on the hi-res provider & download"
        );
    }
}
