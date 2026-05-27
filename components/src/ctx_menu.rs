//! Right-click context menu surface.
//!
//! Sections (top→bottom):
//! 1. Header (title + artist · provider)
//! 2. Playback — Play next, Add to queue
//! 3. Discovery — Song Radio
//! 4. Save — Like / Unlike
//! 5. Navigate — Artist / Album

use std::sync::Arc;

use dioxus::prelude::*;
use hooks::{DiscoveryEngine, use_ctx_menu, use_detail, use_likes, use_queue};

#[component]
pub fn ContextMenu() -> Element {
    let ctx = use_ctx_menu();
    let queue = use_queue();
    let detail = use_detail();
    let likes = use_likes();
    let engine = use_context::<Arc<DiscoveryEngine>>();
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
        .filter(|a| !a.uri.0.is_empty())
        .map(|a| (a.uri.clone(), a.name.clone()));
    let album_nav = track
        .album
        .as_ref()
        .filter(|a| !a.uri.0.is_empty())
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
    let x = state.x;
    let y = state.y;

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
            style: "left: {x}px; top: {y}px;",

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
            }

            div { class: "ctx-sep" }

            div { class: "ctx-group",
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
                        div { class: "ctx-item-sub", "Play 40 similar tracks" }
                    }
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
