//! Home — activity-first dashboard.
//!
//! Three equal-weight sections stacked top-to-bottom: Recently played (local
//! play-log), Recently liked (Spotify Liked Songs sorted by `added_at`), and
//! Listened lately (ListenBrainz `/user/<name>/listens`).
//!
//! Each section degrades to its own empty / error state independently — Home
//! works on a fresh install (everything empty with CTAs) and fills in as
//! providers connect.

use components::{Button, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    DiscoveryResult, HistoryEntry, Listen, Provider, Query, Track, UseFeatured,
    UseLibrary, UseListenBrainzFeed, use_ctx_menu, use_featured, use_history, use_library,
    use_listenbrainz_feed, use_queue, use_soundcloud, use_spotify,
};

#[component]
pub fn Home() -> Element {
    let history = use_history();
    let library = use_library();
    let feed = use_listenbrainz_feed();
    let featured = use_featured(library.clone());

    rsx! {
        section { class: "page home-page",
            h1 { "Home" }
            p { class: "hint",
                "What you've been doing lately — across providers and devices."
            }

            Featured { featured: featured.clone() }
            RecentlyPlayed { entries: history.entries.read().clone() }
            RecentlyLiked { library: library.clone() }
            ListenedLately { feed: feed.clone() }
        }
    }
}

// ─── Featured ("Try this") hero card ──────────────────────────────────────

#[component]
fn Featured(featured: UseFeatured) -> Element {
    let suggestion = featured.suggestion.read().clone();
    let seed = featured.seed.read().clone();
    let is_loading = *featured.is_loading.read();
    let error = featured.error.read().clone();
    let needs_library = *featured.needs_library.read();
    let queue = use_queue();

    rsx! {
        section { class: "home-section featured-section",
            header { class: "home-section-header",
                h2 { "Try this" }
                Button {
                    label: "Surprise me".to_string(),
                    icon: Some("fa-solid fa-shuffle".to_string()),
                    variant: ButtonVariant::Ghost,
                    disabled: needs_library || is_loading,
                    on_click: {
                        let featured = featured.clone();
                        move |_| featured.reroll()
                    },
                }
            }

            if needs_library {
                div { class: "home-empty",
                    div { class: "home-empty-glyph",
                        i { class: "fa-solid fa-wand-magic-sparkles" }
                    }
                    p { class: "home-empty-title", "Like a few songs first." }
                    p { class: "home-empty-body",
                        "Connect Spotify in Settings and like some tracks — nira picks "
                        "a random one and surfaces a similar track from across providers."
                    }
                }
            } else if let Some(rec) = suggestion.as_ref() {
                FeaturedCard {
                    result: rec.clone(),
                    seed: seed.clone(),
                    on_play: {
                        let queue = queue.clone();
                        let rec = rec.clone();
                        move |_| {
                            if let Some(track) = rec.play_target() {
                                queue.play_list(vec![track], 0);
                            }
                        }
                    },
                }
            } else if let Some(msg) = error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else {
                // Catch-all so the section always has visible content. This
                // covers the brief initial mount before `use_effect` fires
                // *and* in-flight reroll where is_loading=true but suggestion
                // hasn't landed yet.
                FeaturedSkeleton { seed: seed.clone() }
            }
        }
    }
}

#[component]
fn FeaturedCard(result: DiscoveryResult, seed: Option<Track>, on_play: EventHandler<()>) -> Element {
    let cover = result.cover_url.clone().unwrap_or_default();
    let title = result.title.clone();
    let artist = result.artist.clone();
    let has_spotify = result.spotify.is_some();
    let has_soundcloud = result.soundcloud.is_some();
    let rationale = result.rationale.clone();
    let seed_label = seed.as_ref().map(|t| {
        let artist = t
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} — {}", artist, t.title)
    });
    let ctx = use_ctx_menu();
    let ctx_target = result.play_target();

    rsx! {
        article {
            class: "featured-card",
            oncontextmenu: {
                let ctx_target = ctx_target.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let Some(t) = ctx_target.clone() else { return; };
                    let pos = e.data.client_coordinates();
                    ctx.open(pos.x, pos.y, t);
                }
            },
            div { class: "featured-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    div { class: "cover-card-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
            }
            div { class: "featured-meta",
                span { class: "featured-eyebrow", "Recommended for you" }
                h3 { class: "featured-title", "{title}" }
                p { class: "featured-artist", "{artist}" }
                if let Some(label) = seed_label.as_ref() {
                    p { class: "featured-rationale",
                        title: "{rationale}",
                        i { class: "fa-solid fa-link" }
                        " based on "
                        span { class: "featured-seed", "{label}" }
                    }
                }
                div { class: "featured-actions",
                    Button {
                        label: "Play".to_string(),
                        icon: Some("fa-solid fa-play".to_string()),
                        variant: ButtonVariant::Primary,
                        on_click: move |_| on_play.call(()),
                    }
                    div { class: "featured-badges",
                        if has_spotify {
                            span { class: "track-badge spotify", "S" }
                        }
                        if has_soundcloud {
                            span { class: "track-badge soundcloud", "SC" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FeaturedSkeleton(seed: Option<Track>) -> Element {
    let seed_label = seed.as_ref().map(|t| {
        format!(
            "{} — {}",
            t.artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            t.title
        )
    });
    rsx! {
        article { class: "featured-card featured-card-skeleton",
            div { class: "featured-art featured-art-skeleton" }
            div { class: "featured-meta",
                span { class: "featured-eyebrow", "Searching neighbours…" }
                div { class: "featured-skeleton-line wide" }
                div { class: "featured-skeleton-line" }
                if let Some(label) = seed_label.as_ref() {
                    p { class: "featured-rationale",
                        i { class: "fa-solid fa-link" }
                        " based on "
                        span { class: "featured-seed", "{label}" }
                    }
                }
            }
        }
    }
}

// ─── Recently played ──────────────────────────────────────────────────────

#[component]
fn RecentlyPlayed(entries: Vec<HistoryEntry>) -> Element {
    rsx! {
        section { class: "home-section",
            header { class: "home-section-header",
                h2 { "Recently played" }
                span { class: "home-section-meta",
                    if entries.is_empty() {
                        "nothing yet"
                    } else {
                        "{entries.len()} most recent"
                    }
                }
            }

            if entries.is_empty() {
                EmptyState {
                    icon: "fa-solid fa-clock-rotate-left",
                    title: "No plays yet.",
                    body: "Hit play in the transport bar (or pick a track from Search / Library) — it shows up here next time you open Home.",
                }
            } else {
                div { class: "cover-row",
                    for entry in entries.iter() {
                        HistoryCard { entry: entry.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn HistoryCard(entry: HistoryEntry) -> Element {
    let queue = use_queue();
    let sc = use_soundcloud();
    let sp = use_spotify();
    let cover = entry.cover_url.clone().unwrap_or_default();
    let badge_class = badge_class_for(&entry.provider);
    let badge = badge_glyph_for(&entry.provider);
    let title = entry.title.clone();
    let artist = entry.artist.clone();
    let played_label = format_relative(entry.played_at);
    let provider = entry.provider.clone();

    rsx! {
        button {
            class: "cover-card clickable",
            title: "{title} — {artist}\nplayed {played_label}",
            onclick: move |_| {
                // Resolve back to a real Track via the original provider's
                // search — we don't store the full URI in the play log, so a
                // round-trip is the price of replaying an old row. First hit
                // wins; same heuristic as Discovery's cross-platform resolve.
                let title = title.clone();
                let artist = artist.clone();
                let provider = provider.clone();
                let queue = queue.clone();
                let sc = sc.clone();
                let sp = sp.clone();
                spawn(async move {
                    let q = Query {
                        text: format!("{} {}", artist, title),
                        limit: Some(5),
                    };
                    let result: Option<Track> = match provider.as_str() {
                        "Spotify" => sp.search(&q).await.ok().and_then(|r| r.tracks.into_iter().next()),
                        "SoundCloud" => sc.search(&q).await.ok().and_then(|r| r.tracks.into_iter().next()),
                        _ => None,
                    };
                    if let Some(track) = result {
                        queue.play_list(vec![track], 0);
                    }
                });
            },
            div { class: "cover-card-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    div { class: "cover-card-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
                span { class: "{badge_class}", "{badge}" }
            }
            div { class: "cover-card-title", "{title}" }
            div { class: "cover-card-sub", "{artist}" }
            div { class: "cover-card-time", "{played_label}" }
        }
    }
}

// ─── Recently liked ────────────────────────────────────────────────────────

#[component]
fn RecentlyLiked(library: UseLibrary) -> Element {
    let tracks = library.recently_liked.read().clone();
    let total_liked = library.liked.read().len();
    let is_loading = *library.is_loading.read();
    let error = library.error.read().clone();
    let queue = use_queue();
    let full_list = library.recently_liked;

    rsx! {
        section { class: "home-section",
            header { class: "home-section-header",
                h2 { "Recently liked" }
                span { class: "home-section-meta",
                    if total_liked > 0 {
                        "{total_liked} liked songs"
                    } else if is_loading {
                        "loading…"
                    } else {
                        ""
                    }
                }
            }

            if let Some(msg) = error.as_ref() {
                div { class: "home-error", "{msg}" }
            } else if tracks.is_empty() && !is_loading {
                EmptyState {
                    icon: "fa-solid fa-heart",
                    title: "No liked songs yet.",
                    body: "Connect Spotify in Settings — your liked-songs library shows up here once the first sync finishes.",
                }
            } else {
                div { class: "cover-row",
                    for (idx, track) in tracks.iter().enumerate() {
                        TrackCard {
                            key: "{track.uri.0}",
                            track: track.clone(),
                            on_click: {
                                let queue = queue.clone();
                                move |_| {
                                    queue.play_list(full_list.read().clone(), idx);
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TrackCard(track: Track, on_click: EventHandler<()>) -> Element {
    let cover = track.cover_url.clone().unwrap_or_default();
    let title = track.title.clone();
    let artist = track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let badge_class = match track.provider {
        hooks::ProviderId::Spotify => "track-badge spotify",
        hooks::ProviderId::SoundCloud => "track-badge soundcloud",
        hooks::ProviderId::Local => "track-badge",
    };
    let ctx = use_ctx_menu();

    rsx! {
        button {
            class: "cover-card clickable",
            title: "{title} — {artist}",
            onclick: move |_| on_click.call(()),
            oncontextmenu: {
                let track = track.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let pos = e.data.client_coordinates();
                    ctx.open(pos.x, pos.y, track.clone());
                }
            },
            div { class: "cover-card-art",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy" }
                } else {
                    div { class: "cover-card-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
                span { class: "{badge_class}", "{track.provider.badge()}" }
            }
            div { class: "cover-card-title", "{title}" }
            div { class: "cover-card-sub", "{artist}" }
        }
    }
}

// ─── Listened lately (ListenBrainz feed) ──────────────────────────────────

#[component]
fn ListenedLately(feed: UseListenBrainzFeed) -> Element {
    let listens = feed.listens.read().clone();
    let is_loading = *feed.is_loading.read();
    let error = feed.error.read().clone();
    let needs_config = *feed.needs_config.read();

    rsx! {
        section { class: "home-section",
            header { class: "home-section-header",
                h2 { "Listened lately" }
                span { class: "home-section-meta",
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

// ─── Shared bits ───────────────────────────────────────────────────────────

#[component]
fn EmptyState(icon: &'static str, title: &'static str, body: &'static str) -> Element {
    rsx! {
        div { class: "home-empty",
            div { class: "home-empty-glyph",
                i { class: "{icon}" }
            }
            p { class: "home-empty-title", "{title}" }
            p { class: "home-empty-body", "{body}" }
        }
    }
}

/// Map a free-text provider/source string to a badge class. We accept the
/// generic `ProviderId::label()` strings ("Spotify"/"SoundCloud") as well as
/// the looser tags ListenBrainz emits in `listening_from` (e.g. "spotify",
/// "lastfm import"). Everything that doesn't match falls back to neutral.
fn badge_class_for(s: &str) -> &'static str {
    let lower = s.to_ascii_lowercase();
    if lower.contains("spotify") {
        "track-badge spotify"
    } else if lower.contains("soundcloud") {
        "track-badge soundcloud"
    } else {
        "track-badge"
    }
}

fn badge_glyph_for(s: &str) -> &'static str {
    let lower = s.to_ascii_lowercase();
    if lower.contains("spotify") {
        "S"
    } else if lower.contains("soundcloud") {
        "SC"
    } else {
        "·"
    }
}

/// Coarse human-readable relative time. Used in both row tooltips and feed
/// lines — anything more precise than minute-of-hour is noise for this
/// surface.
fn format_relative(when: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(when);
    let secs = delta.num_seconds().max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 60 * 60 {
        let m = secs / 60;
        format!("{m} min ago")
    } else if secs < 60 * 60 * 24 {
        let h = secs / 3600;
        format!("{h} h ago")
    } else if secs < 60 * 60 * 24 * 7 {
        let d = secs / 86_400;
        format!("{d} d ago")
    } else {
        when.format("%Y-%m-%d").to_string()
    }
}
