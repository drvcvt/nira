//! Detail-view routing — Artist and Album pages overlay the active section.
//!
//! The shell normally renders one of the five Sections (Home, Discover,
//! Search, Library, Settings). When `current` here is `Some`, the shell
//! renders the detail page *instead* of the Section's main content; the
//! sidebar still shows which Section is selected. This sidesteps a full
//! router refactor for a feature that only has two detail surfaces.

use dioxus::prelude::*;
use provider_api::{AlbumUri, ArtistUri};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailView {
    Artist(ArtistUri),
    Album(AlbumUri),
}

#[derive(Clone, Copy)]
pub struct UseDetail {
    pub current: Signal<Option<DetailView>>,
}

impl UseDetail {
    pub fn open_artist(&self, uri: ArtistUri) {
        let mut current = self.current;
        current.set(Some(DetailView::Artist(uri)));
    }

    pub fn open_album(&self, uri: AlbumUri) {
        let mut current = self.current;
        current.set(Some(DetailView::Album(uri)));
    }

    pub fn close(&self) {
        let mut current = self.current;
        current.set(None);
    }
}

pub fn install_detail() {
    let current = use_signal(|| None::<DetailView>);
    use_context_provider(move || UseDetail { current });
}

pub fn use_detail() -> UseDetail {
    use_context::<UseDetail>()
}
