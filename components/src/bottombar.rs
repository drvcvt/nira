//! Floating player bar. Transport buttons drive the queue (prev/next walk
//! the index, stop clears it); play/pause stays on the player handle since
//! it's transport-only.

use std::time::Duration;

use dioxus::prelude::*;
use hooks::{RepeatMode, Track, use_ctx_menu, use_detail, use_likes, use_player, use_queue};

#[component]
pub fn Bottombar() -> Element {
    let player = use_player();
    let queue = use_queue();
    let likes = use_likes();
    let detail = use_detail();
    let mut queue_open = use_signal(|| false);
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
    let cover_url = np
        .as_ref()
        .and_then(|n| n.cover_url.clone())
        .unwrap_or_default();
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
    let queue_len = queue.entries.read().len();
    let queue_is_open = *queue_open.read();
    let shuffle_on = *queue.shuffle_enabled.read();
    let repeat_mode = *queue.repeat_mode.read();
    let repeat_title = match repeat_mode {
        RepeatMode::Off => "Repeat off",
        RepeatMode::All => "Repeat all",
        RepeatMode::One => "Repeat one",
    };

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
                        class: if shuffle_on { "player-btn active" } else { "player-btn" },
                        title: if shuffle_on { "Shuffle on" } else { "Shuffle off" },
                        disabled: queue_len < 2,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.toggle_shuffle()
                        },
                        i { class: "fa-solid fa-shuffle" }
                    }
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
                    button {
                        class: match repeat_mode {
                            RepeatMode::Off => "player-btn",
                            RepeatMode::All => "player-btn active",
                            RepeatMode::One => "player-btn active repeat-one",
                        },
                        title: "{repeat_title}",
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.cycle_repeat()
                        },
                        i { class: "fa-solid fa-repeat" }
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
                button {
                    class: if queue_is_open { "player-queue-btn open" } else { "player-queue-btn" },
                    title: "Queue",
                    onclick: move |_| queue_open.set(!queue_is_open),
                    i { class: "fa-solid fa-list" }
                    span { "{queue_len}" }
                }
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

            if queue_is_open {
                QueuePopover {}
            }
        }
    }
}

#[component]
fn QueuePopover() -> Element {
    let queue = use_queue();
    let entries = queue.entries.read().clone();
    let current = *queue.current_index.read();
    let total = entries.len();
    let shuffle_on = *queue.shuffle_enabled.read();
    let repeat_mode = *queue.repeat_mode.read();
    let repeat_label = match repeat_mode {
        RepeatMode::Off => "repeat off",
        RepeatMode::All => "repeat all",
        RepeatMode::One => "repeat one",
    };

    rsx! {
        div { class: "queue-popover",
            div { class: "queue-popover-head",
                div {
                    span { class: "queue-eyebrow", "up next" }
                    h3 { "Queue" }
                }
                div { class: "queue-head-actions",
                    if shuffle_on {
                        span { class: "queue-chip on", "shuffle" }
                    }
                    span { class: if repeat_mode == RepeatMode::Off { "queue-chip" } else { "queue-chip on" }, "{repeat_label}" }
                    span { class: "queue-total", "{total} tracks" }
                    button {
                        class: "queue-clear-btn",
                        title: "Clear queue",
                        disabled: entries.is_empty(),
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.stop()
                        },
                        i { class: "fa-solid fa-xmark" }
                    }
                }
            }
            if entries.is_empty() {
                div { class: "queue-empty",
                    i { class: "fa-solid fa-list" }
                    span { "Nothing queued yet." }
                }
            } else {
                ol { class: "queue-list",
                    for (idx, track) in entries.iter().enumerate() {
                        QueueRow {
                            key: "{track.uri.0}-{idx}",
                            track: track.clone(),
                            entries: entries.clone(),
                            index: idx,
                            current: current == Some(idx),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn QueueRow(track: Track, entries: Vec<Track>, index: usize, current: bool) -> Element {
    let queue = use_queue();
    let ctx = use_ctx_menu();
    let cover = track.cover_url.clone().unwrap_or_default();
    let title = track.title.clone();
    let artist = track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let duration = fmt_time(track.duration.as_secs());
    let badge = track.provider.badge();
    let provider = track.provider.label();

    rsx! {
        li {
            class: if current { "queue-row current" } else { "queue-row" },
            title: "{title} — {artist}",
            onclick: {
                let entries = entries.clone();
                let queue = queue.clone();
                move |_| queue.play_context(entries.clone(), index)
            },
            oncontextmenu: {
                let track = track.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    ctx.open(pos.x, pos.y, track.clone());
                }
            },
            span { class: "queue-row-index", "{index + 1}" }
            div { class: "queue-row-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    i { class: "fa-solid fa-music" }
                }
            }
            div { class: "queue-row-copy",
                span { class: "queue-row-title", "{title}" }
                span { class: "queue-row-artist", "{artist}" }
            }
            span { class: "queue-row-badge", "data-provider": "{provider}", "{badge}" }
            span { class: "queue-row-duration", "{duration}" }
        }
    }
}

fn fmt_time(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}
