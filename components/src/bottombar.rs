//! Floating player bar. Transport buttons drive the queue (prev/next walk
//! the index, stop clears it); play/pause stays on the player handle since
//! it's transport-only.

use std::time::Duration;

use dioxus::prelude::*;
use hooks::{use_detail, use_likes, use_player, use_queue};

#[component]
pub fn Bottombar() -> Element {
    let player = use_player();
    let queue = use_queue();
    let likes = use_likes();
    let detail = use_detail();
    // Track corresponding to the current queue index (full Track with
    // URI), needed for the heart toggle in the player-right cluster.
    let current_track = {
        let entries = queue.entries.read();
        let idx = *queue.current_index.read();
        idx.and_then(|i| entries.get(i).cloned())
    };
    let liked_now = current_track
        .as_ref()
        .map(|t| likes.is_liked(&t.uri))
        .unwrap_or(false);
    // Local scrub state — while the user is actively dragging the thumb,
    // we paint the bar from this value instead of the live snapshot so
    // the slider doesn't fight backwards drags. Cleared on pointerup
    // (and as a safety net by a final `onchange`).
    let mut scrub: Signal<Option<f64>> = use_signal(|| None);
    let snap = player.snapshot();
    let volume_pct = (snap.volume * 100.0).round() as i32;

    let np = snap.now_playing.clone();
    let position = snap.position;
    let duration = snap.duration;

    let position_str = fmt_time(position.as_secs());
    let duration_str = duration
        .map(|d| fmt_time(d.as_secs()))
        .unwrap_or_else(|| "--:--".to_string());
    let live_pct = match duration {
        Some(d) if d.as_secs() > 0 => {
            ((position.as_secs_f64() / d.as_secs_f64()) * 100.0).clamp(0.0, 100.0)
        }
        _ => 0.0,
    };
    // Effective progress for rendering. We let `scrub` override `live_pct`
    // until the snapshot catches up to within ~1.5% of where the user
    // dropped the thumb — that's our auto-converge. No explicit cleanup
    // on release; the next render where snapshot agrees naturally falls
    // back to live_pct. The next drag just overwrites `scrub` again.
    let scrub_val: Option<f64> = *scrub.read();
    let progress_pct = match scrub_val {
        Some(t) if (t - live_pct).abs() > 1.5 => t,
        _ => live_pct,
    };

    let now_active = snap.has_source && !snap.is_paused;
    let cover_url = np.as_ref().and_then(|n| n.cover_url.clone()).unwrap_or_default();
    let title_text = np
        .as_ref()
        .map(|n| n.title.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Nothing loaded".to_string());
    let meta_text = np
        .as_ref()
        .map(|n| n.artist.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if snap.has_source {
                "—".to_string()
            } else {
                "press play for a 440 Hz test tone".to_string()
            }
        });
    let source_label = np
        .as_ref()
        .map(|n| n.source_label.clone())
        .unwrap_or_else(|| "—".to_string());
    let provider_attr = np
        .as_ref()
        .map(|n| n.provider.clone())
        .unwrap_or_else(|| "Local".to_string());

    let has_prev = queue.has_previous();
    let has_next = queue.has_next();

    rsx! {
        footer { class: "player",
            div { class: "player-left",
                div { class: "player-art",
                    if !cover_url.is_empty() {
                        img { src: "{cover_url}", alt: "", loading: "lazy" }
                    } else {
                        i { class: "fa-solid fa-music" }
                    }
                }
                div { class: "player-copy",
                    div { class: "player-title-row",
                        span { class: "player-title", "{title_text}" }
                    }
                    // Render the artist line as a clickable link when the
                    // current queue entry exposes an ArtistRef with a URI.
                    // NowPlaying alone only carries a plain string, so we
                    // pick the URI off the entries[current_index] track.
                    {
                        let first_artist = current_track.as_ref()
                            .and_then(|t| t.artists.first().cloned())
                            .filter(|a| !a.uri.0.is_empty());
                        match first_artist {
                            Some(a) => rsx! {
                                div { class: "player-meta",
                                    button {
                                        class: "artist-link",
                                        title: "Go to artist",
                                        onclick: {
                                            let uri = a.uri.clone();
                                            let detail = detail;
                                            move |e: Event<MouseData>| {
                                                e.stop_propagation();
                                                detail.open_artist(uri.clone());
                                            }
                                        },
                                        "{meta_text}"
                                    }
                                }
                            },
                            None => rsx! { div { class: "player-meta", "{meta_text}" } },
                        }
                    }
                }
                button {
                    class: if liked_now { "player-like-btn liked" } else { "player-like-btn" },
                    title: if liked_now { "Remove from Liked" } else { "Save to Liked" },
                    disabled: current_track.is_none(),
                    onclick: {
                        let likes = likes;
                        let track = current_track.clone();
                        move |_| {
                            if let Some(t) = track.as_ref() {
                                likes.toggle(t);
                            }
                        }
                    },
                    if liked_now {
                        i { class: "fa-solid fa-heart" }
                    } else {
                        i { class: "fa-regular fa-heart" }
                    }
                }
            }

            div { class: "player-center",
                div { class: "player-transport",
                    button {
                        class: "player-btn",
                        title: "Previous",
                        disabled: !has_prev,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.previous()
                        },
                        i { class: "fa-solid fa-backward-step" }
                    }
                    button {
                        class: "player-btn play",
                        title: if snap.is_paused { "Resume / play test tone" } else { "Pause" },
                        onclick: {
                            let player = player.clone();
                            move |_| player.toggle()
                        },
                        if now_active {
                            i { class: "fa-solid fa-pause" }
                        } else {
                            i { class: "fa-solid fa-play" }
                        }
                    }
                    button {
                        class: "player-btn",
                        title: "Next",
                        disabled: !has_next,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.next()
                        },
                        i { class: "fa-solid fa-forward-step" }
                    }
                }
                div { class: "player-progress-row",
                    span {
                        class: if now_active { "player-time now active" } else { "player-time now" },
                        "{position_str}"
                    }
                    div {
                        class: "player-progress",
                        style: "--progress: {progress_pct}%;",
                        // Real <input type=range> so we get drag, click,
                        // keyboard arrows, and accessibility for free. The
                        // track + fill + thumb are all styled in main.css
                        // off the `--progress` custom property.
                        input {
                            r#type: "range",
                            class: "player-progress-input",
                            min: "0",
                            max: "1000",
                            step: "1",
                            value: "{(progress_pct * 10.0) as i64}",
                            disabled: duration.is_none() || duration.map(|d| d.as_secs() == 0).unwrap_or(true),
                            "aria-label": "Seek",
                            // wry's webview doesn't fire `change` reliably
                            // for <input type=range>, so we hook `input`
                            // (fires per mousemove tick) and seek live.
                            // The `scrub` signal keeps the bar painted at
                            // the user's drag position until the snapshot
                            // catches up — that's our auto-converge so the
                            // thumb doesn't fight backward drags.
                            oninput: {
                                let player = player.clone();
                                let dur = duration;
                                move |evt: FormEvent| {
                                    let Ok(v) = evt.value().parse::<f64>() else { return; };
                                    let Some(d) = dur else { return; };
                                    let pct = (v / 10.0).clamp(0.0, 100.0);
                                    scrub.set(Some(pct));
                                    let ratio = pct / 100.0;
                                    let target = Duration::from_secs_f64(d.as_secs_f64() * ratio);
                                    player.seek(target);
                                }
                            },
                        }
                    }
                    span { class: "player-time total", "{duration_str}" }
                }
            }

            div { class: "player-right",
                div { class: "player-source",
                    span { class: "player-source-dot", "data-provider": "{provider_attr}" }
                    span { "{source_label}" }
                }
                div { class: "volume",
                    i { class: "vol-icon fa-solid fa-volume-high" }
                    input {
                        r#type: "range",
                        class: "vol-slider",
                        min: "0",
                        max: "100",
                        value: "{volume_pct}",
                        oninput: {
                            let player = player.clone();
                            move |evt: FormEvent| {
                                if let Ok(v) = evt.value().parse::<f32>() {
                                    player.set_volume(v / 100.0);
                                }
                            }
                        }
                    }
                    span { class: "vol-pct", "{volume_pct}" }
                }
            }
        }
    }
}

fn fmt_time(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}
