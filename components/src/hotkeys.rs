//! Global keyboard shortcuts + the Ctrl+/ shortcut sheet.
//!
//! Key events are caught by a capture-phase document listener installed by
//! the shell (`nira/src/main.rs`) — the only place that sees keys regardless
//! of focus. It clicks the invisible bridge buttons below; those carry the
//! actual Dioxus handlers (same mechanism as the search hotkey). This
//! component owns the bridges and the shortcut-sheet overlay.

use dioxus::prelude::*;
use hooks::{use_config, use_likes, use_player, use_queue};

use crate::visualizer::use_viz_open;

/// Volume step per Ctrl+↑/↓ press, in slider units (0..1).
const VOLUME_STEP: f32 = 0.05;

type BindRows = &'static [(&'static [&'static str], &'static str)];

const BINDS_PLAYBACK: BindRows = &[
    (&["Space"], "Play / Pause"),
    (&["Ctrl + ←"], "Previous track"),
    (&["Ctrl + →"], "Next track"),
    (&["S"], "Toggle shuffle"),
    (&["R"], "Cycle repeat"),
    (&["L"], "Like current track"),
];
const BINDS_VOLUME: BindRows = &[
    (&["Ctrl + ↑"], "Volume up"),
    (&["Ctrl + ↓"], "Volume down"),
];
const BINDS_APP: BindRows = &[
    (&["Ctrl + F", "Alt + Space"], "Search"),
    (&["V"], "Visualizer"),
    (&["Ctrl + /"], "Keyboard shortcuts"),
    (&["Esc"], "Close overlays"),
];

#[component]
pub fn Hotkeys() -> Element {
    let player = use_player();
    let queue = use_queue();
    let likes = use_likes();
    let config = use_config();
    let mut viz_open = use_viz_open().0;
    let mut open = use_signal(|| false);
    let is_open = *open.read();

    let overlay_class = if is_open {
        "binds-overlay open"
    } else {
        "binds-overlay"
    };

    // Current queue entry (full Track) for the Like bind.
    let current_track = {
        let entries = queue.entries.read();
        let idx = *queue.current_index.read();
        idx.and_then(|i| entries.get(i).cloned())
    };

    rsx! {
        // — bridge buttons, clicked from the shell's JS key listener —
        button {
            id: "nira-key-playpause",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: {
                let player = player.clone();
                let queue = queue.clone();
                move |_| {
                    if player.toggle() {
                        return;
                    }
                    // Nothing loaded — start the queue where it points
                    // (same behaviour as the transport play button).
                    let idx = (*queue.current_index.peek()).unwrap_or(0);
                    queue.play_index(idx);
                }
            },
        }
        button {
            id: "nira-key-next",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: {
                let queue = queue.clone();
                move |_| queue.next()
            },
        }
        button {
            id: "nira-key-prev",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: {
                let queue = queue.clone();
                move |_| queue.previous()
            },
        }
        button {
            id: "nira-key-shuffle",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: {
                let queue = queue.clone();
                move |_| queue.toggle_shuffle()
            },
        }
        button {
            id: "nira-key-repeat",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: {
                let queue = queue.clone();
                move |_| queue.cycle_repeat()
            },
        }
        button {
            id: "nira-key-like",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: move |_| {
                if let Some(t) = current_track.as_ref() {
                    likes.toggle(t);
                }
            },
        }
        button {
            id: "nira-key-volup",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: {
                let player = player.clone();
                move |_| {
                    // config.volume is written synchronously by set_volume,
                    // so it's always current — unlike the polled snapshot.
                    let v = config.read().volume;
                    player.set_volume(v + VOLUME_STEP);
                }
            },
        }
        button {
            id: "nira-key-voldown",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: {
                let player = player.clone();
                move |_| {
                    let v = config.read().volume;
                    player.set_volume(v - VOLUME_STEP);
                }
            },
        }
        button {
            id: "nira-key-viz",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: move |_| {
                let now = *viz_open.peek();
                viz_open.set(!now);
            },
        }
        button {
            id: "nira-key-viz-close",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: move |_| viz_open.set(false),
        }
        button {
            id: "nira-key-binds",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: move |_| {
                let now = *open.peek();
                open.set(!now);
            },
        }
        button {
            id: "nira-key-binds-close",
            class: "hotkey-bridge",
            r#type: "button",
            tabindex: "-1",
            onclick: move |_| open.set(false),
        }

        // — the shortcut sheet —
        div { class: "{overlay_class}",
            button {
                class: "binds-backdrop",
                r#type: "button",
                tabindex: "-1",
                onclick: move |_| open.set(false),
            }
            div { class: "binds-panel",
                div { class: "binds-head",
                    span { class: "binds-eyebrow", "keyboard" }
                    h3 { "Shortcuts" }
                    span { class: "binds-hint",
                        kbd { "Esc" }
                        " close"
                    }
                }
                div { class: "binds-groups",
                    {bind_group("Playback", BINDS_PLAYBACK)}
                    {bind_group("Volume", BINDS_VOLUME)}
                    {bind_group("App", BINDS_APP)}
                }
            }
        }
    }
}

fn bind_group(title: &str, rows: BindRows) -> Element {
    rsx! {
        div { class: "binds-group",
            div { class: "binds-group-title", "{title}" }
            for (keys, action) in rows.iter() {
                div { class: "binds-row",
                    span { class: "binds-action", "{action}" }
                    span { class: "binds-keys",
                        for k in keys.iter() {
                            kbd { "{k}" }
                        }
                    }
                }
            }
        }
    }
}
