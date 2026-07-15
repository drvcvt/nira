//! The shared rail shell plus shelf/mix rows — every horizontal row on Home
//! goes through `Rail` so headers, reroll buttons and spacing stay uniform.

use std::sync::atomic::{AtomicUsize, Ordering};

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{RecommendationMix, RecommendationShelf, Track, UseRecommendations, use_queue};

use super::TrackCard;

/// Monotonic id so each rail's header arrows can address exactly its own
/// scroll row via a DOM selector. Never reset — uniqueness is all we need.
static RAIL_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Scroll a rail's row by ~a viewport, smoothly. `dir` is -1 (left) / 1
/// (right). The row's own scrollbar is hidden in CSS — these arrows (and
/// trackpad/wheel gestures) are the scrolling affordance.
fn scroll_rail(rail_id: &str, dir: i32) {
    document::eval(&format!(
        "var el = document.querySelector('#{rail_id} .home-rail-row'); \
         if (el) el.scrollBy({{left: {dir} * Math.max(240, el.clientWidth * 0.85), behavior: 'smooth'}});"
    ));
}

#[component]
pub(super) fn Rail(
    eyebrow: String,
    title: String,
    #[props(default)] sub: String,
    #[props(default)] reroll_id: Option<String>,
    #[props(default)] recommendations: Option<UseRecommendations>,
    #[props(default = false)] rerolling: bool,
    children: Element,
) -> Element {
    let rail_id = use_hook(|| format!("home-rail-{}", RAIL_SEQ.fetch_add(1, Ordering::Relaxed)));
    rsx! {
        section { class: "home-rail", id: "{rail_id}",
            header { class: "home-rail-head",
                div { class: "home-rail-titles",
                    span { class: "rail-eyebrow", "{eyebrow}" }
                    h3 { class: "home-rail-title", "{title}" }
                    if !sub.is_empty() {
                        p { class: "home-rail-sub", "{sub}" }
                    }
                }
                div { class: "home-rail-actions",
                    if let (Some(id), Some(rec)) = (reroll_id.clone(), recommendations.clone()) {
                        Button {
                            label: if rerolling { "Rerolling".to_string() } else { "Reroll".to_string() },
                            icon: Some(if rerolling { "fa-solid fa-circle-notch fa-spin".to_string() } else { "fa-solid fa-shuffle".to_string() }),
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            disabled: rerolling,
                            on_click: move |_| rec.reroll_shelf(id.clone()),
                        }
                    }
                    button {
                        class: "rail-scroll-btn",
                        title: "Scroll left",
                        onclick: {
                            let rail_id = rail_id.clone();
                            move |_| scroll_rail(&rail_id, -1)
                        },
                        i { class: "fa-solid fa-chevron-left" }
                    }
                    button {
                        class: "rail-scroll-btn",
                        title: "Scroll right",
                        onclick: {
                            let rail_id = rail_id.clone();
                            move |_| scroll_rail(&rail_id, 1)
                        },
                        i { class: "fa-solid fa-chevron-right" }
                    }
                }
            }
            {children}
        }
    }
}

#[component]
pub(super) fn ShelfRail(shelf: RecommendationShelf, recommendations: UseRecommendations) -> Element {
    let tracks = shelf.tracks.clone();
    let reroll_id = if shelf.rerollable {
        Some(shelf.id.clone())
    } else {
        None
    };

    rsx! {
        Rail {
            eyebrow: shelf.eyebrow.clone(),
            title: shelf.title.clone(),
            sub: shelf.subtitle.clone(),
            reroll_id,
            recommendations: Some(recommendations.clone()),
            rerolling: shelf.is_loading,
            if let Some(msg) = shelf.error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else if shelf.is_loading && tracks.is_empty() {
                SkeletonRow {}
            } else if tracks.is_empty() {
                div { class: "shelf-empty", "Nothing here yet." }
            } else {
                div { class: "home-rail-row",
                    for (idx, track) in tracks.iter().enumerate() {
                        TrackCard {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            tracks: tracks.clone(),
                            index: idx,
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn DailyMixesRail(
    mixes: Vec<RecommendationMix>,
    recommendations: UseRecommendations,
) -> Element {
    let any_loading = mixes.iter().any(|m| m.is_loading);
    rsx! {
        Rail {
            eyebrow: "Clusters".to_string(),
            title: "Daily Mixes".to_string(),
            reroll_id: Some("daily-mixes".to_string()),
            recommendations: Some(recommendations.clone()),
            rerolling: any_loading,
            div { class: "home-rail-row",
                for mix in mixes.iter() {
                    DailyMixCard { key: "{mix.id}", mix: mix.clone() }
                }
            }
        }
    }
}

#[component]
fn DailyMixCard(mix: RecommendationMix) -> Element {
    let queue = use_queue();
    let tracks = mix.tracks.clone();
    let artworks = mix_artworks(&tracks);

    rsx! {
        button {
            class: "cover-card mix-card clickable",
            r#type: "button",
            title: "{mix.title} — {mix.subtitle}",
            disabled: tracks.is_empty() || mix.is_loading,
            onclick: move |_| queue.play_context(tracks.clone(), 0),
            div { class: "cover-card-art mix-card-art",
                if mix.is_loading && artworks.is_empty() {
                    div { class: "cover-card-fallback", i { class: "fa-solid fa-circle-notch fa-spin" } }
                } else if artworks.len() >= 4 {
                    div { class: "mix-mosaic",
                        for src in artworks.iter().take(4) {
                            div { key: "{src}", class: "mix-mosaic-cell", style: "background-image: url('{src}')" }
                        }
                    }
                } else if let Some(src) = artworks.first() {
                    img { src: "{src}", alt: "", loading: "lazy", decoding: "async" }
                } else {
                    div { class: "cover-card-fallback", i { class: "fa-solid fa-music" } }
                }
            }
            div { class: "cover-card-title", "{mix.title}" }
            div { class: "cover-card-sub", "{mix.subtitle}" }
        }
    }
}

#[component]
pub(super) fn SkeletonRow() -> Element {
    rsx! {
        div { class: "home-rail-row for-you-loading-row",
            for idx in 0..6 {
                div { key: "{idx}", class: "cover-card for-you-skeleton-card",
                    div { class: "cover-card-art" }
                    div { class: "for-you-skeleton-line wide" }
                    div { class: "for-you-skeleton-line" }
                }
            }
        }
    }
}

fn mix_artworks(tracks: &[Track]) -> Vec<String> {
    let mut seen = std::collections::HashSet::<String>::new();
    let mut out = Vec::new();
    for track in tracks {
        let Some(url) = track.cover_url.as_ref() else {
            continue;
        };
        if seen.insert(url.clone()) {
            out.push(url.clone());
        }
        if out.len() >= 4 {
            break;
        }
    }
    out
}
