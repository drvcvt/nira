//! Right-click context menu — global UI singleton.
//!
//! One menu is open at a time. Rows on any page open it with a track + the
//! click coordinates; the `ContextMenu` component (in `components/`) reads
//! the same signal and renders itself at that position. Closing happens
//! either via overlay click, Escape, or after an action runs.
//!
//! Kept in `hooks/` (not in `components/`) because the *state* is global
//! whereas the rendering is a component — the same split as `use_player`.

use dioxus::prelude::*;
use provider_api::Track;

/// One open-menu invocation. Carries everything the menu needs to render
/// itself without re-borrowing the row that opened it.
#[derive(Clone, Debug, PartialEq)]
pub struct CtxMenuState {
    pub x: f64,
    pub y: f64,
    pub track: Track,
}

#[derive(Clone, Copy)]
pub struct UseCtxMenu {
    pub current: Signal<Option<CtxMenuState>>,
}

impl UseCtxMenu {
    pub fn open(&self, x: f64, y: f64, track: Track) {
        let mut current = self.current;
        current.set(Some(CtxMenuState { x, y, track }));
    }

    pub fn close(&self) {
        let mut current = self.current;
        current.set(None);
    }
}

/// Install the singleton signal into Dioxus context. Call once from
/// `AppContext::install`. The `ContextMenu` component and any track row
/// then both reach for it via `use_ctx_menu`.
pub fn install_ctx_menu() {
    let current = use_signal(|| None::<CtxMenuState>);
    use_context_provider(move || UseCtxMenu { current });
}

pub fn use_ctx_menu() -> UseCtxMenu {
    use_context::<UseCtxMenu>()
}
