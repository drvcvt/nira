//! Centralized search input bar — the singleton for any page that needs
//! a text query input. Two shape variants (`Rounded` for inline use,
//! `Pill` for prominent search pages), optional prefix icon, and an
//! optional `children` slot rendered inside the bar's border for hint
//! chips, spinners, or kbd shortcuts. Sibling controls (Discover's "Find
//! similar" Button) live OUTSIDE the SearchBar — use `.searchbar-row`
//! as the layout container.

use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchBarShape {
    /// Border-radius `var(--r-btn)`. For inline use alongside other controls.
    Rounded,
    /// Full pill — border-radius `var(--r-full)`. For prominent search inputs.
    Pill,
}

#[component]
pub fn SearchBar(
    value: String,
    placeholder: String,
    on_input: EventHandler<String>,
    /// Fires when the user presses Enter inside the input.
    #[props(default)] on_submit: Option<EventHandler<()>>,
    #[props(default = SearchBarShape::Rounded)] shape: SearchBarShape,
    /// Font Awesome (or any) class for an inline prefix glyph.
    #[props(default)] icon: Option<String>,
    /// Trailing content rendered inside the bar after the input — hint
    /// chip, spinner, kbd shortcut. Sibling buttons go OUTSIDE.
    children: Element,
    #[props(default = false)] autofocus: bool,
) -> Element {
    let shape_class = match shape {
        SearchBarShape::Rounded => "searchbar-rounded",
        SearchBarShape::Pill => "searchbar-pill",
    };
    let class = format!("searchbar {shape_class}");

    rsx! {
        div { class: "{class}",
            if let Some(icon) = icon.as_ref() {
                i { class: "searchbar-icon {icon}" }
            }
            input {
                r#type: "text",
                class: "searchbar-input",
                placeholder: "{placeholder}",
                value: "{value}",
                oninput: move |e| on_input.call(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter {
                        if let Some(cb) = on_submit.as_ref() {
                            cb.call(());
                        }
                    }
                },
                autofocus,
            }
            {children}
        }
    }
}
