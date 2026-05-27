use crate::Section;
use dioxus::prelude::*;

/// A `(section, label, font-awesome icon)` triple. Grouped into kopuz-style
/// uppercase section headers so the sidebar visually distinguishes
/// "discovery surfaces" from "library surfaces".
#[derive(PartialEq, Clone)]
struct NavItem {
    section: Section,
    label: &'static str,
    icon: &'static str,
}

const DISCOVERY: &[NavItem] = &[
    NavItem {
        section: Section::Home,
        label: "Home",
        icon: "fa-solid fa-house",
    },
    NavItem {
        section: Section::Discover,
        label: "Discover",
        icon: "fa-solid fa-compass",
    },
];

const LIBRARY: &[NavItem] = &[NavItem {
    section: Section::Library,
    label: "Library",
    icon: "fa-solid fa-music",
}];

const TOOLS: &[NavItem] = &[NavItem {
    section: Section::Settings,
    label: "Settings",
    icon: "fa-solid fa-gear",
}];

#[component]
pub fn Sidebar(section: Signal<Section>) -> Element {
    rsx! {
        aside { class: "sidebar",
            div { class: "sidebar-brand", "nira" }

            div { class: "sidebar-section-label", "discover" }
            NavList { items: DISCOVERY, section }

            div { class: "sidebar-section-label", "library" }
            NavList { items: LIBRARY, section }

            div { class: "sidebar-divider" }
            NavList { items: TOOLS, section }
        }
    }
}

#[component]
fn NavList(items: &'static [NavItem], section: Signal<Section>) -> Element {
    rsx! {
        nav { class: "side-nav",
            for item in items {
                button {
                    class: if *section.read() == item.section { "nav-item active" } else { "nav-item" },
                    onclick: {
                        let mut section = section;
                        let s = item.section;
                        move |_| section.set(s)
                    },
                    span { class: "nav-glyph",
                        i { class: "{item.icon}" }
                    }
                    span { "{item.label}" }
                }
            }
        }
    }
}
