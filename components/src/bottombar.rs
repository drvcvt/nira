//! Floating player bar. Transport buttons drive the queue (prev/next walk
//! the index, stop clears it); play/pause stays on the player handle since
//! it's transport-only.

use std::time::{Duration, Instant};

use dioxus::prelude::*;
use hooks::{
    RadioStatus, RepeatMode, Track, fmt_time, use_ctx_menu, use_detail, use_likes, use_player,
    use_queue,
};

use crate::cover::use_cover_open;
use crate::visualizer::use_viz_open;

#[component]
pub fn Bottombar() -> Element {
    let player = use_player();
    let queue = use_queue();
    let likes = use_likes();
    let detail = use_detail();
    let mut queue_open = use_signal(|| false);
    let mut viz_open = use_viz_open().0;
    let mut cover_open = use_cover_open().0;
    let mute_stash = crate::hotkeys::use_mute_stash().0;
    // Volume comes from config, not from the player snapshot: the snapshot is
    // written by a 200/500 ms poller, so rendering the slider off it made the
    // thumb fight the drag, and the mute button stashed a stale level (drag
    // down, mute within 200 ms, unmute → jumps back to the OLD volume).
    // `set_volume` writes config synchronously — same source the M-key uses.
    let config = hooks::use_config();
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
    // the slider doesn't fight backwards drags. Tagged with the track URI
    // it was made for: a skip changes the URI, so a stale drag value can
    // never paint the bar for the next track. After release it holds the
    // drop point until the janitor effect below clears it — it must NOT
    // linger past that: an uncleaned scrub used to re-win the paint as
    // soon as playback drifted >1.5% past the drop point, freezing the
    // fill there for the rest of the track (labels kept counting).
    let mut scrub: Signal<Option<(String, f64)>> = use_signal(|| None);
    // True between pointerdown and pointerup on the slider. While set, the
    // bar paints from `scrub` unconditionally so the still-advancing engine
    // snapshot cannot fight a backwards drag.
    let mut scrub_dragging = use_signal(|| false);
    // When the latest input dispatched a seek. Drives the janitor's
    // failed-seek backstop: no convergence within 3 s → drop the hold and
    // show the honest live position again.
    let mut scrub_committed: Signal<Option<Instant>> = use_signal(|| None);
    // Hold-release janitor: runs on every engine tick. Once the engine's
    // position lands within 1.5% of the held drop point — or the backstop
    // expires — clear the hold so live progress owns the bar again.
    {
        let player = player.clone();
        use_effect(move || {
            let snap = player.snapshot(); // subscribes this effect to engine ticks
            if *scrub_dragging.peek() {
                return;
            }
            let Some((_, target)) = scrub.peek().clone() else {
                return;
            };
            let live = snap
                .duration
                .filter(|d| d.as_secs() > 0)
                .map(|d| (snap.position.as_secs_f64() / d.as_secs_f64()) * 100.0)
                .unwrap_or(0.0);
            // No commit timestamp while not dragging means a stale local
            // value survived without a seek — clear it now.
            let expired = (*scrub_committed.peek())
                .map(|t0| t0.elapsed() > Duration::from_secs(3))
                .unwrap_or(true);
            if (target - live).abs() <= 1.5 || expired {
                scrub.set(None);
                scrub_committed.set(None);
            }
        });
    }
    let snap = player.snapshot();
    let volume_pct = (config.read().volume * 100.0).round() as i32;

    let np = snap.now_playing.clone();
    // While the next queue entry is being fetched (SC download, librespot
    // connect) the engine still reports the *old* track's position/duration.
    // Blank the bar instead of showing stale progress under the new title.
    let track_loading = *queue.is_loading_track.read();
    let position = snap.position;
    let duration = if track_loading { None } else { snap.duration };
    // Identity of what the bar currently represents; scrub values are only
    // honoured while this stays the same.
    let track_key = np
        .as_ref()
        .and_then(|n| n.track_uri.clone())
        .unwrap_or_default();

    let position_str = if track_loading {
        "0:00".to_string()
    } else {
        fmt_time(position.as_secs())
    };
    let duration_str = duration
        .map(|d| fmt_time(d.as_secs()))
        .unwrap_or_else(|| "--:--".to_string());
    let live_pct = match duration {
        Some(d) if d.as_secs() > 0 => {
            ((position.as_secs_f64() / d.as_secs_f64()) * 100.0).clamp(0.0, 100.0)
        }
        _ => 0.0,
    };
    // Effective progress for rendering: `scrub` wins while it exists —
    // during a drag it follows the pointer, after release it holds the
    // drop point until the janitor effect clears it (seek landed or
    // backstop expired). Track changes invalidate it via the key.
    let scrub_val: Option<f64> = scrub
        .read()
        .as_ref()
        .filter(|(key, _)| *key == track_key)
        .map(|(_, v)| *v);
    let progress_pct = scrub_val.unwrap_or(live_pct);

    let now_active = snap.has_source && !snap.is_paused;
    let transport_locked = snap.transport_locked;
    let queue_len = queue.entries.read().len();
    // With nothing loaded but a (restored) queue pointing somewhere, show
    // that entry's identity instead of a blank "Nothing loaded" — the play
    // button/Space will start exactly this track.
    let idle_track = (!snap.has_source && np.is_none())
        .then(|| current_track.clone())
        .flatten();
    let cover_url = np
        .as_ref()
        .and_then(|n| n.cover_url.clone())
        .or_else(|| idle_track.as_ref().and_then(|t| t.cover_url.clone()))
        .unwrap_or_default();
    let title_text = np
        .as_ref()
        .map(|n| n.title.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| idle_track.as_ref().map(|t| t.title.clone()))
        .unwrap_or_else(|| "Nothing loaded".to_string());
    let meta_text = np
        .as_ref()
        .map(|n| n.artist.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if !snap.has_source && queue_len > 0 {
                "press play to start the queue".to_string()
            } else {
                "—".to_string()
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
    let track_error = queue.error.read().clone();
    let radio_status = queue.radio_status.read().clone();
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
                // The art doubles as the fullscreen-player trigger; hover
                // shows the expand affordance (styles in cover.css).
                button {
                    class: "player-art",
                    title: "Full-screen player",
                    "aria-label": "Open full-screen player",
                    onclick: move |_| cover_open.set(true),
                    if !cover_url.is_empty() {
                        img { src: "{cover_url}", alt: "", loading: "lazy", decoding: "async" }
                    } else {
                        i { class: "fa-solid fa-music" }
                    }
                    span { class: "player-art-expand",
                        i { class: "fa-solid fa-expand" }
                    }
                }
                div { class: "player-copy",
                    div { class: "player-title-row",
                        // Tooltip carries the full text — the span ellipsizes
                        // on long titles.
                        span { class: "player-title", title: "{title_text}", "{title_text}" }
                    }
                    // Render the artist line as a clickable link when the
                    // current queue entry exposes an ArtistRef with a URI.
                    // NowPlaying alone only carries a plain string, so we
                    // pick the URI off the entries[current_index] track.
                    {
                        let first_artist = current_track.as_ref()
                            .and_then(|t| t.artists.first().cloned())
                            .filter(|a| hooks::uri_has_detail_page(&a.uri.0));
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
                    "aria-label": if liked_now { "Remove from Liked" } else { "Save to Liked" },
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
                        "aria-label": if shuffle_on { "Shuffle on" } else { "Shuffle off" },
                        "aria-pressed": if shuffle_on { "true" } else { "false" },
                        disabled: transport_locked || queue_len < 2,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.toggle_shuffle()
                        },
                        i { class: "fa-solid fa-shuffle" }
                    }
                    button {
                        class: "player-btn",
                        title: if transport_locked { "Following host" } else { "Previous" },
                        "aria-label": if transport_locked { "Following host" } else { "Previous track" },
                        disabled: transport_locked || !has_prev,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.previous()
                        },
                        i { class: "fa-solid fa-backward-step" }
                    }
                    button {
                        class: "player-btn play",
                        title: if transport_locked { "Following host" } else if now_active { "Pause" } else { "Play" },
                        "aria-label": if transport_locked { "Following host" } else if now_active { "Pause" } else { "Play" },
                        disabled: transport_locked,
                        onclick: {
                            let player = player.clone();
                            let queue = queue.clone();
                            move |_| {
                                if player.toggle() {
                                    return;
                                }
                                // Nothing loaded — start the queue if it has
                                // entries ("Add to queue" while idle, retry
                                // after a failed load). No-op on empty queue.
                                let idx = (*queue.current_index.peek()).unwrap_or(0);
                                queue.play_index(idx);
                            }
                        },
                        if now_active {
                            i { class: "fa-solid fa-pause" }
                        } else {
                            i { class: "fa-solid fa-play" }
                        }
                    }
                    button {
                        class: "player-btn",
                        title: if transport_locked { "Following host" } else { "Next" },
                        "aria-label": if transport_locked { "Following host" } else { "Next track" },
                        disabled: transport_locked || !has_next,
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
                        "aria-label": "{repeat_title}",
                        disabled: transport_locked,
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
                        style: "--seek-pct: {progress_pct}%;",
                        // Real <input type=range> so we get drag, click,
                        // keyboard arrows, and accessibility for free. The
                        // track + fill + thumb are all styled in player.css
                        // off the `--seek-pct` custom property.
                        input {
                            r#type: "range",
                            class: "player-progress-input",
                            min: "0",
                            max: "1000",
                            step: "1",
                            value: "{(progress_pct * 10.0) as i64}",
                            disabled: transport_locked || duration.is_none() || duration.map(|d| d.as_secs() == 0).unwrap_or(true),
                            title: if transport_locked { "Following host" } else { "Seek" },
                            "aria-label": if transport_locked { "Following host" } else { "Seek" },
                            onpointerdown: move |_| {
                                scrub.set(None);
                                scrub_dragging.set(true);
                            },
                            onpointerup: move |_| scrub_dragging.set(false),
                            onpointercancel: move |_| {
                                scrub_dragging.set(false);
                                scrub.set(None);
                            },
                            oninput: {
                                let player = player.clone();
                                let dur = duration;
                                let track_key = track_key.clone();
                                move |evt: FormEvent| {
                                    let Ok(v) = evt.value().parse::<f64>() else { return; };
                                    let Some(d) = dur else { return; };
                                    let pct = (v / 10.0).clamp(0.0, 100.0);
                                    scrub.set(Some((track_key.clone(), pct)));
                                    let target =
                                        Duration::from_secs_f64(d.as_secs_f64() * pct / 100.0);
                                    player.seek(target);
                                    scrub_committed.set(Some(Instant::now()));
                                }
                            },
                        }
                    }
                    span { class: "player-time total", "{duration_str}" }
                }
            }

            div { class: "player-right",
                button {
                    class: if *viz_open.read() { "player-viz-btn open" } else { "player-viz-btn" },
                    title: "Visualizer (V)",
                    "aria-label": "Visualizer",
                    "aria-pressed": if *viz_open.read() { "true" } else { "false" },
                    onclick: move |_| {
                        let now = *viz_open.peek();
                        viz_open.set(!now);
                    },
                    i { class: "fa-solid fa-atom" }
                }
                button {
                    class: if queue_is_open { "player-queue-btn open" } else { "player-queue-btn" },
                    title: "Queue",
                    "aria-label": "Queue ({queue_len} tracks)",
                    "aria-expanded": if queue_is_open { "true" } else { "false" },
                    onclick: move |_| queue_open.set(!queue_is_open),
                    i { class: "fa-solid fa-list" }
                    span { "{queue_len}" }
                }
                // Bridge for the shell's Escape handler — the popover has no
                // reliable focus, so its dismiss key lives at the document
                // listener like every other overlay.
                button {
                    id: "nira-key-queue-close",
                    class: "hotkey-bridge",
                    r#type: "button",
                    tabindex: "-1",
                    onclick: move |_| queue_open.set(false),
                }
                div { class: "player-source",
                    span { class: "player-source-dot", "data-provider": "{provider_attr}" }
                    span { "{source_label}" }
                }
                div { class: "volume",
                    button {
                        class: "vol-mute-btn",
                        title: if volume_pct == 0 { "Unmute (M)" } else { "Mute (M)" },
                        "aria-label": if volume_pct == 0 { "Unmute" } else { "Mute" },
                        onclick: {
                            let player = player.clone();
                            let mut stash = mute_stash;
                            move |_| {
                                let v = config.read().volume;
                                crate::hotkeys::toggle_mute(&player, v, &mut stash);
                            }
                        },
                        if volume_pct == 0 {
                            i { class: "vol-icon fa-solid fa-volume-xmark" }
                        } else {
                            i { class: "vol-icon fa-solid fa-volume-high" }
                        }
                    }
                    input {
                        r#type: "range",
                        class: "vol-slider",
                        min: "0",
                        max: "100",
                        value: "{volume_pct}",
                        "aria-label": "Volume",
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
                QueuePopover { on_close: move |_| queue_open.set(false) }
            }

            // Track-load / radio status, rendered globally: the bar is the
            // one surface that exists on every page, so a failed load is
            // never invisible just because the user browsed somewhere else.
            // One toast at a time; load errors outrank radio chatter.
            if let Some(err) = track_error.as_ref() {
                div { class: "playback-toast",
                    i { class: "fa-solid fa-circle-exclamation playback-toast-glyph" }
                    span { class: "playback-toast-msg", "{err}" }
                    button {
                        class: "download-toast-close",
                        title: "Dismiss",
                        "aria-label": "Dismiss",
                        onclick: {
                            let mut error = queue.error;
                            move |_| error.set(None)
                        },
                        i { class: "fa-solid fa-xmark" }
                    }
                }
            } else if radio_status == RadioStatus::Loading {
                div { class: "playback-toast",
                    i { class: "fa-solid fa-circle-notch fa-spin playback-toast-glyph" }
                    span { class: "playback-toast-msg", "Song Radio — finding similar tracks…" }
                }
            } else if let RadioStatus::Error(msg) = &radio_status {
                div { class: "playback-toast",
                    i { class: "fa-solid fa-circle-exclamation playback-toast-glyph" }
                    span { class: "playback-toast-msg", "Song Radio failed ({msg}) — playing the seed only." }
                    button {
                        class: "download-toast-close",
                        title: "Dismiss",
                        "aria-label": "Dismiss",
                        onclick: {
                            let mut status = queue.radio_status;
                            move |_| status.set(RadioStatus::Idle)
                        },
                        i { class: "fa-solid fa-xmark" }
                    }
                }
            }
        }
    }
}

/// Rows rendered per window step. After "shuffle all" the queue holds
/// thousands of entries; rendering them all froze the popover open.
// ponytail: grow-on-demand window, real virtualization if scroll-jumping
// through 10k-row queues ever becomes a workflow.
const QUEUE_WINDOW: usize = 250;

#[component]
fn QueuePopover(on_close: EventHandler<()>) -> Element {
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
    // Window: a little history, then QUEUE_WINDOW rows from just above the
    // current track; "show more" extends downward.
    let mut extra = use_signal(|| 0usize);
    let start = current.unwrap_or(0).saturating_sub(25);
    let end = (start + QUEUE_WINDOW + *extra.read()).min(total);

    rsx! {
        // Click-outside catcher — same pattern as the ctx-menu overlay.
        button {
            class: "queue-overlay",
            r#type: "button",
            tabindex: "-1",
            "aria-hidden": "true",
            onclick: move |_| on_close.call(()),
        }
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
                    // Destructive action gets a WORD, not an icon — the old
                    // xmark here read as "close panel" and ate whole queues.
                    button {
                        class: "queue-clear-btn",
                        title: "Clear upcoming (keeps the playing track)",
                        disabled: entries.is_empty(),
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.clear_upcoming()
                        },
                        "clear"
                    }
                    button {
                        class: "queue-close-btn",
                        title: "Close",
                        "aria-label": "Close queue",
                        onclick: move |_| on_close.call(()),
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
                    if start > 0 {
                        li { class: "queue-window-note", "{start} earlier tracks" }
                    }
                    for (idx, track) in entries.iter().enumerate().skip(start).take(end - start) {
                        QueueRow {
                            key: "{track.uri.0}-{idx}",
                            track: track.clone(),
                            index: idx,
                            current: current == Some(idx),
                        }
                    }
                    if end < total {
                        li { class: "queue-window-note",
                            button {
                                class: "queue-window-more",
                                onclick: move |_| {
                                    let now = *extra.peek();
                                    extra.set(now + QUEUE_WINDOW);
                                },
                                "Show more ({total - end} left)"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn QueueRow(track: Track, index: usize, current: bool) -> Element {
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
            // Keyboard-reachable: the row acts as a play button.
            tabindex: "0",
            role: "button",
            // Jump within the existing queue — play_list would re-seed and,
            // with shuffle on, scramble the whole visible order per click.
            onclick: {
                let queue = queue.clone();
                move |_| queue.play_index(index)
            },
            onkeydown: {
                let queue = queue.clone();
                move |e: Event<KeyboardData>| {
                    let key = e.key();
                    let is_space = key.to_string() == " ";
                    if key == Key::Enter || is_space {
                        e.prevent_default();
                        if is_space {
                            e.stop_propagation();
                        }
                        queue.play_index(index);
                    }
                }
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
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
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
            button {
                class: "queue-row-remove",
                r#type: "button",
                title: "Remove from queue",
                "aria-label": "Remove {title} from queue",
                onkeydown: |e: KeyboardEvent| e.stop_propagation(),
                onclick: {
                    let queue = queue.clone();
                    move |e: Event<MouseData>| {
                        e.stop_propagation();
                        queue.remove_at(index);
                    }
                },
                i { class: "fa-solid fa-xmark" }
            }
        }
    }
}
