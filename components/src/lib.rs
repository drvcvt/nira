//! Reusable UI surfaces shared between pages.
//!
//! Components are pure presentational — they take signals/props and emit
//! events, they don't reach into global state or talk to the audio engine.
//! That's the `hooks` crate's job.

pub mod bottombar;
pub mod button;
pub mod ctx_menu;
pub mod download_toast;
pub mod hotkeys;
pub mod searchbar;
pub mod sidebar;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use download_toast::DownloadToast;
pub use searchbar::SearchBar;

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
