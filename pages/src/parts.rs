//! Small render-only helpers shared across pages.
//!
//! Lives at the page layer (not in `components/`) because everything
//! here is page-scoped chrome — track rows, inline links — and pages
//! already depend on `hooks`, so no extra dep wiring is needed.

use dioxus::prelude::*;
use hooks::{AlbumRef, ArtistRef, use_detail};

/// Renders `track.artists` as a comma-separated list where each name is
/// a button that opens the artist detail page. Clicks stop propagation
/// so the surrounding row's `onclick` (typically Play) doesn't fire.
/// Artists without a URI render as plain spans.
#[component]
pub fn ArtistLinks(artists: Vec<ArtistRef>) -> Element {
    let detail = use_detail();
    rsx! {
        for (idx, artist) in artists.iter().enumerate() {
            if idx > 0 {
                span { class: "artist-link-sep", ", " }
            }
            if artist.uri.0.is_empty() {
                span { "{artist.name}" }
            } else {
                button {
                    class: "artist-link",
                    onclick: {
                        let uri = artist.uri.clone();
                        let detail = detail;
                        move |e: Event<MouseData>| {
                            e.stop_propagation();
                            detail.open_artist(uri.clone());
                        }
                    },
                    "{artist.name}"
                }
            }
        }
    }
}

/// Inline album-name button mirroring [`ArtistLinks`]. Pages that show
/// `Track.album` use this to give a one-click jump to the album page.
#[component]
pub fn AlbumLink(album: AlbumRef) -> Element {
    let detail = use_detail();
    let title = album.title.clone();
    if album.uri.0.is_empty() {
        rsx! { span { "{title}" } }
    } else {
        rsx! {
            button {
                class: "artist-link album-link",
                onclick: {
                    let uri = album.uri.clone();
                    let detail = detail;
                    move |e: Event<MouseData>| {
                        e.stop_propagation();
                        detail.open_album(uri.clone());
                    }
                },
                "{title}"
            }
        }
    }
}
