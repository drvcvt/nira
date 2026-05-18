//! Plain `<button>` styled in CSS (main.css `.sq-btn` rules).
//!
//! Why no SVG: at the sizes we use (Md ≈ 38 px, Sm ≈ 28 px) with a 12 px
//! radius, `border-radius` is visually indistinguishable from an Apple
//! squircle. Where the engine supports the W3C `corner-shape` property
//! (Chromium 142+, Safari TP, webkitgtk follows) we ask for a real
//! squircle for free; everywhere else we fall back to a quarter-circle
//! rounded rect. No SVG, no async measurement, no donut hack.

use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonVariant {
    Primary,
    Ghost,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonSize {
    /// Compact — back buttons, settings rows, secondary actions.
    Sm,
    /// Default — primary CTAs.
    Md,
}

#[component]
pub fn Button(
    label: String,
    on_click: EventHandler<()>,
    #[props(default)] icon: Option<String>,
    #[props(default = ButtonVariant::Primary)] variant: ButtonVariant,
    #[props(default = ButtonSize::Md)] size: ButtonSize,
    #[props(default = false)] disabled: bool,
) -> Element {
    let variant_class = match variant {
        ButtonVariant::Primary => "sq-btn-primary",
        ButtonVariant::Ghost => "sq-btn-ghost",
    };
    let size_class = match size {
        ButtonSize::Sm => "sq-sm",
        ButtonSize::Md => "sq-md",
    };
    let class = format!("sq-btn {variant_class} {size_class}");

    rsx! {
        button {
            class: "{class}",
            disabled,
            onclick: move |_| on_click.call(()),
            if let Some(icon) = icon.as_ref() {
                i { class: "{icon}" }
            }
            "{label}"
        }
    }
}
