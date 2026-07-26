//! Reusable UI surfaces shared between pages.
//!
//! Components are pure presentational — they take signals/props and emit
//! events, they don't reach into global state or talk to the audio engine.
//! That's the `hooks` crate's job.

pub mod bottombar;
pub mod button;
pub mod cover;
pub mod ctx_menu;
pub mod download_toast;
pub mod hotkeys;
pub mod searchbar;
pub mod sidebar;
pub mod visualizer;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use download_toast::DownloadToast;
pub use searchbar::SearchBar;

/// Move focus into an overlay when it opens and hand it back when it closes.
///
/// Focus lives in the DOM, not in any signal we own, so this has to run
/// webview-side. Without the hand-back, closing an overlay dropped focus onto
/// `<body>` — which also silently killed the shell's Rust `onkeydown`, since
/// body isn't a descendant of the shell div.
///
/// The stash is a stack because overlays nest (cover → visualizer): each open
/// pushes, each close pops its own entry rather than clobbering a shared slot.
pub fn overlay_focus(open: bool, first_selector: &str) {
    let js = if open {
        format!(
            "(function(){{\
                (window.__niraFocusStack = window.__niraFocusStack || [])\
                    .push(document.activeElement);\
                requestAnimationFrame(function(){{\
                    var el = document.querySelector('{first_selector}');\
                    if (el) el.focus();\
                }});\
            }})();"
        )
    } else {
        "(function(){\
            var stack = window.__niraFocusStack || [];\
            var el = stack.pop();\
            if (el && el.isConnected && el.focus) el.focus({ preventScroll: true });\
        })();"
            .to_string()
    };
    dioxus::document::eval(&js);
}

/// Which top-level view is currently active.
///
/// Lives here (next to `Sidebar`) because the sidebar is the canonical
/// switcher; pages read it indirectly via the route they're mounted under.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Home,
    Discover,
    Library,
    Settings,
}
