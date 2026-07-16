//! The one expressive "Made for you" stage moment at the top of Home.

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{RecommendationShelf, Track, UseRecommendations, use_queue};

use super::TrackCard;
use crate::parts::TrackCtx;

#[component]
pub(super) fn HomeStage(
    shelf: Option<RecommendationShelf>,
    /// Memoized merged pool — a Memo prop so Home's frequent re-renders
    /// don't clone the full track vector every frame.
    pool: Memo<Vec<Track>>,
    recommendations: UseRecommendations,
    is_loading: bool,
) -> Element {
    let queue = use_queue();

    let tracks = shelf.as_ref().map(|s| s.tracks.clone()).unwrap_or_default();
    let tracks_ctx = TrackCtx::new(tracks.clone());
    let title = shelf
        .as_ref()
        .map(|s| s.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "For You".to_string());
    let seed = shelf.as_ref().map(|s| s.seed_label.clone()).unwrap_or_default();
    let error = shelf.as_ref().and_then(|s| s.error.clone());
    let lead = tracks.first().cloned();
    let pool_len = pool.read().len();
    let has_pool = pool_len > 0;

    rsx! {
        section { class: "home-stage",
            div { class: "home-stage-art",
                if let Some(track) = lead.clone() {
                    TrackCard { track: track.clone(), tracks: tracks_ctx.clone(), index: 0 }
                } else {
                    div { class: "cover-card",
                        div { class: "cover-card-art",
                            div { class: "cover-card-fallback",
                                i {
                                    class: if is_loading { "fa-solid fa-circle-notch fa-spin" } else { "fa-solid fa-wand-magic-sparkles" },
                                }
                            }
                        }
                    }
                }
            }
            div { class: "home-stage-body",
                h2 { class: "home-stage-title", "{title}" }
                if !seed.is_empty() {
                    p { class: "home-stage-seed",
                        "based on "
                        span { "{seed}" }
                    }
                }
                p { class: "home-stage-sub",
                    if is_loading && !has_pool {
                        "Building your mixes and picks…"
                    } else if !has_pool {
                        "Play or like a few tracks — nira builds your Home here."
                    } else {
                        "{pool_len} tracks across mixes, related picks and scenes."
                    }
                }
                div { class: "home-stage-actions",
                    Button {
                        label: "Play".to_string(),
                        icon: Some("fa-solid fa-play".to_string()),
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Sm,
                        disabled: tracks.is_empty(),
                        on_click: {
                            let queue = queue.clone();
                            let tracks = tracks.clone();
                            move |_| queue.play_context(tracks.clone(), 0)
                        },
                    }
                    Button {
                        label: "Shuffle".to_string(),
                        icon: Some("fa-solid fa-shuffle".to_string()),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: !has_pool,
                        on_click: {
                            let queue = queue.clone();
                            move |_| queue.play_context(pool(), 0)
                        },
                    }
                    Button {
                        label: "Surprise".to_string(),
                        icon: Some("fa-solid fa-dice".to_string()),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: !has_pool,
                        on_click: {
                            let queue = queue.clone();
                            move |_| {
                                let pool = pool();
                                if pool.is_empty() {
                                    return;
                                }
                                let idx = (chrono::Utc::now().timestamp_millis().unsigned_abs()
                                    as usize)
                                    % pool.len();
                                queue.play_context(pool, idx);
                            }
                        },
                    }
                    Button {
                        label: "Refresh".to_string(),
                        icon: Some(if is_loading { "fa-solid fa-circle-notch fa-spin".to_string() } else { "fa-solid fa-rotate".to_string() }),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: is_loading,
                        on_click: {
                            let recommendations = recommendations.clone();
                            move |_| recommendations.refresh_all()
                        },
                    }
                }
                if let Some(msg) = error.as_ref() {
                    div { class: "home-error", "{msg}" }
                } else if tracks.len() > 1 {
                    div { class: "home-stage-picks",
                        for (idx, track) in tracks.iter().enumerate().skip(1).take(4) {
                            TrackCard {
                                key: "{track.uri.0}",
                                track: track.clone(),
                                tracks: tracks_ctx.clone(),
                                index: idx,
                            }
                        }
                    }
                }
            }
        }
    }
}
