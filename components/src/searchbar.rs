//! Centralized search input bar — the single visual implementation for any
//! page that needs a text query input. It supports an optional prefix icon and
//! an optional `children` slot rendered inside the bar's border for hint chips,
//! spinners, or kbd shortcuts. Sibling controls (Discover's "Find similar"
//! Button) live OUTSIDE the SearchBar — use `.searchbar-row` as the layout
//! container.

use dioxus::prelude::*;

#[component]
pub fn SearchBar(
    value: String,
    placeholder: String,
    on_input: EventHandler<String>,
    /// Fires when the user presses Enter inside the input.
    #[props(default)]
    on_submit: Option<EventHandler<()>>,
    /// Font Awesome (or any) class for an inline prefix glyph.
    #[props(default)]
    icon: Option<String>,
    /// Trailing content rendered inside the bar after the input — hint
    /// chip, spinner, kbd shortcut. Sibling buttons go OUTSIDE.
    children: Element,
    #[props(default = false)] autofocus: bool,
) -> Element {
    rsx! {
        div { class: "searchbar",
            if let Some(icon) = icon.as_ref() {
                i { class: "searchbar-icon {icon}" }
            }
            input {
                r#type: "text",
                class: "searchbar-input",
                placeholder: "{placeholder}",
                "aria-label": "{placeholder}",
                value: "{value}",
                oninput: move |e| on_input.call(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter
                        && let Some(cb) = on_submit.as_ref()
                    {
                        cb.call(());
                    }
                },
                onmounted: move |e: Event<MountedData>| {
                    if autofocus {
                        spawn(async move {
                            let _ = e.data.set_focus(true).await;
                        });
                    }
                },
                autofocus,
            }
            {children}
        }
    }
}
