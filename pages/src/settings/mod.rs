//! Settings — one module per section so each stays small and reviewable.
//! A category nav on the left switches the visible section; the shared
//! SettingsCard building block lives here and section components pull it
//! in via `super::`.

mod appearance;
mod connections;
mod data;
mod discovery;
mod library;

use dioxus::prelude::*;

use appearance::AppearanceSettings;
use connections::{MusicSettings, ProviderSettings};
use data::DataSettings;
use discovery::DiscoverySettings;
use library::LibrarySettings;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Music,
    Providers,
    Theme,
    Discovery,
    Data,
}

const TABS: &[(SettingsTab, &str, &str)] = &[
    (SettingsTab::Music, "Music", "fa-solid fa-music"),
    (SettingsTab::Providers, "Providers", "fa-solid fa-plug"),
    (SettingsTab::Theme, "Theme", "fa-solid fa-palette"),
    (SettingsTab::Discovery, "Discovery", "fa-solid fa-compass"),
    (SettingsTab::Data, "Data", "fa-solid fa-database"),
];

#[component]
pub fn Settings() -> Element {
    let mut tab = use_signal(|| SettingsTab::Music);
    let active = *tab.read();

    rsx! {
        section { class: "page settings-page",
            h1 { "Settings" }

            div { class: "settings-shell",
                nav { class: "settings-nav",
                    for (t, label, icon) in TABS.iter().copied() {
                        button {
                            key: "{label}",
                            class: if active == t { "settings-nav-item active" } else { "settings-nav-item" },
                            onclick: move |_| tab.set(t),
                            span { class: "settings-nav-glyph",
                                i { class: "{icon}" }
                            }
                            span { "{label}" }
                        }
                    }
                }

                div { class: "settings-content",
                    match active {
                        SettingsTab::Music => rsx! {
                            MusicSettings {}
                            LibrarySettings {}
                        },
                        SettingsTab::Providers => rsx! { ProviderSettings {} },
                        SettingsTab::Theme => rsx! { AppearanceSettings {} },
                        SettingsTab::Discovery => rsx! { DiscoverySettings {} },
                        SettingsTab::Data => rsx! { DataSettings {} },
                    }
                }
            }
        }
    }
}

#[component]
fn SettingsCard(title: String, icon: String, children: Element) -> Element {
    rsx! {
        article { class: "settings-card",
            header { class: "settings-card-head",
                div { class: "settings-titleline",
                    i { class: "{icon}" }
                    h3 { "{title}" }
                }
            }
            div { class: "settings-card-body", {children} }
        }
    }
}

/// Small dot+label pill reflecting the *active* state of a feature (as
/// opposed to the drafts sitting in the inputs above it).
#[component]
fn StatusPill(label: String, ok: bool) -> Element {
    let class = if ok {
        "settings-pill ok"
    } else {
        "settings-pill"
    };
    rsx! {
        span { class: "{class}",
            span { class: "settings-dot" }
            "{label}"
        }
    }
}
