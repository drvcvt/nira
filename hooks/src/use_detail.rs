//! Detail-view routing — Artist and Album pages overlay the active section.
//!
//! The shell normally renders one of the five Sections (Home, Discover,
//! Search, Library, Settings). When the stack here is non-empty, the shell
//! renders the top detail page over the Section's main content (which stays
//! mounted, hidden, so its state survives); the sidebar still shows which
//! Section is selected. Opens push onto the stack, so artist → album → Back
//! returns to the artist instead of dumping the user at the section root.
//! This sidesteps a full router refactor for a feature that only has two
//! detail surfaces.

use dioxus::prelude::*;
use provider_api::{AlbumUri, ArtistUri};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailView {
    Artist(ArtistUri),
    Album(AlbumUri),
}

#[derive(Clone, Copy)]
pub struct UseDetail {
    /// Navigation stack; the last entry is on screen. Empty = section shown.
    stack: Signal<Vec<DetailView>>,
}

impl UseDetail {
    /// The detail view currently on screen, if any. Reading subscribes the
    /// calling component to navigation changes.
    pub fn current(&self) -> Option<DetailView> {
        self.stack.read().last().cloned()
    }

    /// The full stack, bottom → top. The shell renders every entry (hidden
    /// except the top) so covered pages keep their state — tab selection,
    /// loaded data — and Back is instant instead of a refetch.
    pub fn views(&self) -> Vec<DetailView> {
        self.stack.read().clone()
    }

    pub fn open_artist(&self, uri: ArtistUri) {
        self.push(DetailView::Artist(uri));
    }

    pub fn open_album(&self, uri: AlbumUri) {
        self.push(DetailView::Album(uri));
    }

    fn push(&self, view: DetailView) {
        let mut stack = self.stack;
        let mut s = stack.peek().clone();
        // Re-opening what's already on screen must not grow the stack —
        // otherwise Back appears to do nothing.
        if s.last() == Some(&view) {
            return;
        }
        s.push(view);
        stack.set(s);
    }

    /// Pop one level: album → the artist it was opened from → section root.
    pub fn back(&self) {
        let mut stack = self.stack;
        let mut s = stack.peek().clone();
        if s.pop().is_some() {
            stack.set(s);
        }
    }

    /// Leave detail navigation entirely (sidebar section click).
    pub fn close(&self) {
        let mut stack = self.stack;
        if !stack.peek().is_empty() {
            stack.set(Vec::new());
        }
    }
}

/// Whether an artist/album URI can actually be opened as a detail page.
/// `local:album:` resolves from the scanned library; `local:artist:` is a
/// name-derived grouping key with no page behind it — UIs render those as
/// plain text instead of links that dead-end on an error page.
pub fn uri_has_detail_page(uri: &str) -> bool {
    uri.starts_with("spotify:")
        || uri.starts_with("soundcloud:")
        || uri.starts_with("hires-provider:")
        || uri.starts_with("local:album:")
}

pub fn install_detail() {
    let stack = use_signal(Vec::<DetailView>::new);
    use_context_provider(move || UseDetail { stack });
}

pub fn use_detail() -> UseDetail {
    use_context::<UseDetail>()
}
