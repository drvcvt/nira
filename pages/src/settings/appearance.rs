//! Appearance — theme (System/Light/Dark) and UI font, both applied live by
//! the shell effect in nira/src/main.rs and persisted in the config.

use dioxus::prelude::*;
use hooks::{ThemePref, UI_FONTS, ui_font_stack, use_config};

#[component]
pub(super) fn AppearanceSettings() -> Element {
    let mut config = use_config();
    let current_theme = config.read().theme;
    let current_font = config
        .read()
        .ui_font
        .clone()
        .unwrap_or_else(|| "geist".to_string());

    let mut pick_theme = move |t: ThemePref| {
        let mut cfg = config.write();
        if cfg.theme != t {
            cfg.theme = t;
            let _ = cfg.save();
        }
    };

    let mut font_open = use_signal(|| false);
    let mut pick_font = move |key: &'static str| {
        let next = if key == "geist" {
            None
        } else {
            Some(key.to_string())
        };
        let mut cfg = config.write();
        if cfg.ui_font != next {
            cfg.ui_font = next;
            let _ = cfg.save();
        }
    };

    let specimen_stack = ui_font_stack(Some(current_font.as_str()));
    let current_font_label = UI_FONTS
        .iter()
        .find(|(k, _)| *k == current_font)
        .map(|(_, l)| *l)
        .unwrap_or("Geist");

    rsx! {
        section { class: "settings-group settings-stack",
            h2 { "Appearance" }
            p { class: "hint",
                "Theme and UI font apply immediately. System theme follows the desktop colour scheme."
            }

            div { class: "settings-row",
                label { "Theme" }
                div { class: "mode-toggle",
                    button {
                        class: if current_theme == ThemePref::System { "mode-btn active" } else { "mode-btn" },
                        onclick: move |_| pick_theme(ThemePref::System),
                        "System"
                    }
                    button {
                        class: if current_theme == ThemePref::Light { "mode-btn active" } else { "mode-btn" },
                        onclick: move |_| pick_theme(ThemePref::Light),
                        "Light"
                    }
                    button {
                        class: if current_theme == ThemePref::Dark { "mode-btn active" } else { "mode-btn" },
                        onclick: move |_| pick_theme(ThemePref::Dark),
                        "Dark"
                    }
                }
            }

            div { class: "settings-row",
                label { "UI font" }
                div { class: "field-dropdown",
                    // Trigger shows the current font, rendered in that font.
                    button {
                        class: if *font_open.read() { "field-dropdown-trigger open" } else { "field-dropdown-trigger" },
                        style: "font-family: {specimen_stack};",
                        onclick: move |_| font_open.toggle(),
                        span { class: "field-dropdown-value", "{current_font_label}" }
                        i { class: "fa-solid fa-chevron-down field-dropdown-chevron" }
                    }
                    if *font_open.read() {
                        // Click-away overlay + the option list, each in its own font.
                        button {
                            class: "field-dropdown-overlay",
                            onclick: move |_| font_open.set(false),
                        }
                        div { class: "field-dropdown-panel",
                            for (key, label) in UI_FONTS.iter() {
                                button {
                                    key: "{key}",
                                    class: if *key == current_font { "field-dropdown-option active" } else { "field-dropdown-option" },
                                    style: "font-family: {ui_font_stack(Some(key))};",
                                    onclick: move |_| { pick_font(key); font_open.set(false); },
                                    span { class: "field-dropdown-option-name", "{label}" }
                                    span { class: "field-dropdown-option-sample", "Aa Gg 0123" }
                                    if *key == current_font {
                                        i { class: "fa-solid fa-check field-dropdown-option-check" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "font-specimen", style: "font-family: {specimen_stack};",
                p { class: "font-specimen-title", "Semantics: The Benzo Chronicles" }
                p { class: "font-specimen-body",
                    "Goreshit — Burn This Moment Into the Retina of My Eye. Pack my box with five dozen liquor jugs."
                }
                p { class: "font-specimen-nums", "0123456789 · 3:42 / 61:08 · 44.1 kHz FLAC" }
            }
        }
    }
}
