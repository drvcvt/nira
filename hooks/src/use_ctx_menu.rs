//! Right-click context menu — global UI singleton.
//!
//! One menu is open at a time. Rows on any page open it with a target + the
//! click coordinates; the `ContextMenu` component (in `components/`) reads
//! the same signal and renders itself at that position. Closing happens
//! either via overlay click, Escape, or after an action runs.
//!
//! Targets: a single track (the common case) or a whole album (album cards
//! and the album-page banner) — the album variant renders its own, smaller
//! menu.
//!
//! Kept in `hooks/` (not in `components/`) because the *state* is global
//! whereas the rendering is a component — the same split as `use_player`.

use dioxus::prelude::*;
use player::HistoryEntry;
use provider_api::Track;

/// Album payload for an album right-click. Tracks are resolved at open time
/// (local scan / already-loaded album detail) so the menu can play or add
/// to a playlist without another fetch.
#[derive(Clone, Debug, PartialEq)]
pub struct AlbumCtx {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub cover_url: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CtxTarget {
    Track(Track),
    Album(AlbumCtx),
}

/// One open-menu invocation. Carries everything the menu needs to render
/// itself without re-borrowing the row that opened it.
#[derive(Clone, Debug, PartialEq)]
pub struct CtxMenuState {
    pub x: f64,
    pub y: f64,
    pub target: CtxTarget,
    /// Set when the menu was opened from a Recently-played card — enables
    /// the "Remove from history" entry for exactly that log row.
    pub history_entry: Option<HistoryEntry>,
}

#[derive(Clone, Copy)]
pub struct UseCtxMenu {
    pub current: Signal<Option<CtxMenuState>>,
}

impl UseCtxMenu {
    pub fn open(&self, x: f64, y: f64, track: Track) {
        let mut current = self.current;
        current.set(Some(CtxMenuState {
            x,
            y,
            target: CtxTarget::Track(track),
            history_entry: None,
        }));
    }

    /// Open for a history card: same track menu, plus a "Remove from
    /// history" entry bound to `entry`.
    pub fn open_for_history(&self, x: f64, y: f64, track: Track, entry: HistoryEntry) {
        let mut current = self.current;
        current.set(Some(CtxMenuState {
            x,
            y,
            target: CtxTarget::Track(track),
            history_entry: Some(entry),
        }));
    }

    /// Open the album variant (album cards, album-page banner).
    pub fn open_album(&self, x: f64, y: f64, album: AlbumCtx) {
        let mut current = self.current;
        current.set(Some(CtxMenuState {
            x,
            y,
            target: CtxTarget::Album(album),
            history_entry: None,
        }));
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
