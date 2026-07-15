//! Listened lately — the ListenBrainz scrobble timeline that closes Home.

use dioxus::prelude::*;
use hooks::{Listen, UseListenBrainzFeed};

use super::{EmptyState, badge_class_for, badge_glyph_for, format_relative};

#[component]
pub(super) fn ListenedLately(feed: UseListenBrainzFeed) -> Element {
    let listens = feed.listens.read().clone();
    let is_loading = *feed.is_loading.read();
    let error = feed.error.read().clone();
    let needs_config = *feed.needs_config.read();

    rsx! {
        section { class: "home-rail home-feed",
            header { class: "home-rail-head",
                div { class: "home-rail-titles",
                    span { class: "rail-eyebrow", "Across players" }
                    h3 { class: "home-rail-title", "Listened lately" }
                }
                span { class: "home-rail-meta",
                    if needs_config {
                        "ListenBrainz off"
                    } else if is_loading {
                        "fetching…"
                    } else if listens.is_empty() {
                        "no listens yet"
                    } else {
                        "{listens.len()} latest"
                    }
                }
            }

            if needs_config {
                EmptyState {
                    icon: "fa-solid fa-rss",
                    title: "Connect ListenBrainz.",
                    body: "Add your ListenBrainz username (and optionally a token for scrobbling) in Settings. Your listening trail across players surfaces here once configured.",
                }
            } else if let Some(msg) = error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else if listens.is_empty() && !is_loading {
                EmptyState {
                    icon: "fa-solid fa-rss",
                    title: "No listens reported yet.",
                    body: "Scrobble a few plays — they'll show up here on the next refresh.",
                }
            } else {
                ul { class: "feed-list",
                    for listen in listens.iter() {
                        FeedLine { listen: listen.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn FeedLine(listen: Listen) -> Element {
    let badge_class = badge_class_for(listen.source.as_deref().unwrap_or(""));
    let badge = badge_glyph_for(listen.source.as_deref().unwrap_or(""));
    let when = format_relative(listen.listened_at);
    let source_label = listen.source.clone().unwrap_or_else(|| "elsewhere".into());

    rsx! {
        li { class: "feed-line",
            span { class: "{badge_class} feed-line-dot", "{badge}" }
            div { class: "feed-line-meta",
                span { class: "feed-line-title", "{listen.title}" }
                span { class: "feed-line-artist", " — {listen.artist}" }
            }
            span { class: "feed-line-source", "{source_label}" }
            span { class: "feed-line-time", "{when}" }
        }
    }
}
