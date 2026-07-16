//! Discover — orbital layout. The page tells the story of nira's
//! three-source merge: SoundCloud's related-tracks feed, the
//! ListenBrainz similarity graph, and Last.fm tag overlap. The empty
//! state is an animated orbit so a fresh launch sells the concept;
//! once results land, the seed promotes itself to a banner and each
//! row exposes which of the three sources voted for it.

use components::{Button, ButtonSize, ButtonVariant, SearchBar};
use dioxus::prelude::*;
use hooks::{
    CrossPlatformMatch, DiscoveryMode, DiscoveryResult, Track, use_ctx_menu, use_discovery,
    use_queue,
};

use crate::parts::{ArtistLinks, PlayableButton, TrackCtx, open_track_context};

#[component]
pub fn Discover() -> Element {
    let mut disc = use_discovery();
    let queue = use_queue();
    let input_value = disc.input.read().clone();
    let results = disc.results.read().clone();
    let is_searching = *disc.is_searching.read();
    let is_loading_track = *queue.is_loading_track.read();
    let disc_error = disc.error.read().clone();
    let has_input = !input_value.trim().is_empty();
    let has_results = !results.is_empty();
    let bridge_match = disc.bridge.read().clone();
    let current_mode = *disc.mode.read();

    // Flatten to playable Tracks for the queue. Discovery results carry both
    // provider variants; we pick whichever the result's `play_target()` says.
    let playable: Vec<Track> = results.iter().filter_map(|r| r.play_target()).collect();

    // Aggregate per-source contribution counts so the seed banner can tell
    // the user *why* the result list looks the way it does (SC-dominant,
    // LB-only, LF off, etc). Folding an empty Vec is free, so we compute
    // unconditionally rather than gating on has_results.
    let (sc_n, lb_n, lf_n) = results.iter().fold((0u32, 0u32, 0u32), |(s, l, f), r| {
        let (a, b, c) = sources_from_rationale(&r.rationale);
        (s + a as u32, l + b as u32, f + c as u32)
    });
    let lf_configured = disc.lastfm_configured();
    let source_prefs = disc.source_prefs();

    rsx! {
        section { class: "page discover-page",
            h1 { "Discover" }

            div { class: "discover-modes",
                button {
                    class: if current_mode == DiscoveryMode::SimilarTo {
                        "discover-mode-tab active"
                    } else {
                        "discover-mode-tab"
                    },
                    onclick: {
                        let disc = disc.clone();
                        move |_| disc.mode.clone().set(DiscoveryMode::SimilarTo)
                    },
                    "Similar to"
                }
                button {
                    class: if current_mode == DiscoveryMode::CrossPlatformBridge {
                        "discover-mode-tab active"
                    } else {
                        "discover-mode-tab"
                    },
                    onclick: {
                        let disc = disc.clone();
                        move |_| disc.mode.clone().set(DiscoveryMode::CrossPlatformBridge)
                    },
                    "Cross-platform bridge"
                }
            }

            div { class: "searchbar-row",
                SearchBar {
                    icon: Some("fa-solid fa-magnifying-glass".to_string()),
                    value: input_value.clone(),
                    placeholder: "artist - title   (e.g. Burial - Archangel)".to_string(),
                    on_input: move |v: String| disc.input.set(v),
                    on_submit: {
                        let disc = disc.clone();
                        move |_| disc.run()
                    },
                    autofocus: true,
                }
                Button {
                    label: if is_searching { "Searching".to_string() } else { "Find similar".to_string() },
                    icon: Some(if is_searching { "fa-solid fa-circle-notch fa-spin".to_string() } else { "fa-solid fa-compass".to_string() }),
                    variant: ButtonVariant::Primary,
                    disabled: is_searching || !has_input,
                    on_click: {
                        let disc = disc.clone();
                        move |_| disc.run()
                    },
                }
            }

            // Bridge-mode hint is non-obvious enough to deserve a line;
            // Similar-to is self-explanatory once the orbit explains the sources.
            if current_mode == DiscoveryMode::CrossPlatformBridge {
                p { class: "discover-hint",
                    "Run Similar-to first, then click a result to resolve it on the other platform — useful for jumping a SoundCloud find onto Spotify or vice versa."
                }
            }

            if is_loading_track {
                p { class: "discover-hint",
                    i { class: "fa-solid fa-circle-notch fa-spin" }
                    " loading track…"
                }
            }

            if let Some(err) = disc_error.as_ref() {
                div { class: "search-error", "{err}" }
            }

            // ── Main stage ─────────────────────────────────────────────
            // Results state wins; otherwise show the orbital canvas
            // (in idle or searching mode). Bridge results render below
            // either way.

            if has_results {
                DiscoverSeedBanner {
                    seed_text: input_value.clone(),
                    count: results.len(),
                    sc_count: sc_n,
                    lb_count: lb_n,
                    lf_count: lf_n,
                    sc_enabled: source_prefs.soundcloud,
                    lb_enabled: source_prefs.listenbrainz,
                    lf_enabled: source_prefs.lastfm,
                    lf_configured,
                    mode: current_mode,
                    on_reroll: {
                        let disc = disc.clone();
                        move |_| disc.run()
                    },
                }

                ul { class: "discovery-list",
                    for r in results.iter() {
                        DiscoveryRow {
                            key: "{r.mbid.clone().unwrap_or_default()}-{r.title}",
                            result: r.clone(),
                            on_play: {
                                let title = r.title.clone();
                                let artist = r.artist.clone();
                                let playable = playable.clone();
                                let queue = queue.clone();
                                let disc = disc.clone();
                                let result_for_bridge = r.clone();
                                move |_| {
                                    match current_mode {
                                        DiscoveryMode::SimilarTo => {
                                            if let Some(p_idx) = playable.iter().position(|t|
                                                t.title == title && t.artists.iter().any(|a| a.name == artist))
                                            {
                                                queue.play_list(playable.clone(), p_idx);
                                            }
                                        }
                                        DiscoveryMode::CrossPlatformBridge => {
                                            if let Some(t) = result_for_bridge.play_target() {
                                                disc.bridge_from_track(t);
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                }
            } else {
                OrbitalCanvas { searching: is_searching, has_input }
            }

            if let Some(m) = bridge_match.as_ref() {
                BridgeResult { bridge: m.clone() }
            }
        }
    }
}

/// Empty / searching state — the orbital scene. Three rings, three
/// dots, a pulsing core, and a status indicator that flips between
/// "STANDBY" and "QUERYING" once Enter is pressed.
#[component]
fn OrbitalCanvas(searching: bool, has_input: bool) -> Element {
    let canvas_class = if searching {
        "discover-canvas searching"
    } else {
        "discover-canvas"
    };

    let core_label = if searching {
        "querying…"
    } else {
        "drop a seed"
    };
    let core_hint = if searching {
        "fanning out across three similarity graphs"
    } else if has_input {
        "press enter to fan out"
    } else {
        "type artist · title and press enter"
    };
    let status_text = if searching { "querying" } else { "standby" };

    rsx! {
        div { class: canvas_class,
            // Three concentric rings. Each carries one rotating dot —
            // the rotation lives on .orbit-spinner so the dashed ring
            // itself stays put.
            div { class: "orbit orbit-lf",
                div { class: "orbit-spinner",
                    div { class: "orbit-dot" }
                }
            }
            div { class: "orbit orbit-lb",
                div { class: "orbit-spinner",
                    div { class: "orbit-dot" }
                }
            }
            div { class: "orbit orbit-sc",
                div { class: "orbit-spinner",
                    div { class: "orbit-dot" }
                }
            }

            // Pulsing core in the dead centre — where the seed would go.
            div { class: "orbit-core",
                div { class: "orbit-core-glyph",
                    i { class: "fa-solid fa-compass" }
                }
                div { class: "orbit-core-label", "{core_label}" }
                div { class: "orbit-core-hint", "{core_hint}" }
            }

            // Legend bottom-left explains the colour code; the dots
            // reappear next to each row in the results state so the
            // user reads the same vocabulary twice.
            div { class: "orbit-legend",
                div { class: "orbit-legend-row", "data-src": "sc",
                    div { class: "orbit-legend-dot" }
                    span { class: "orbit-legend-name", "soundcloud" }
                    span { class: "orbit-legend-tail", "related" }
                }
                div { class: "orbit-legend-row", "data-src": "lb",
                    div { class: "orbit-legend-dot" }
                    span { class: "orbit-legend-name", "listenbrainz" }
                    span { class: "orbit-legend-tail", "similarity graph" }
                }
                div { class: "orbit-legend-row", "data-src": "lf",
                    div { class: "orbit-legend-dot" }
                    span { class: "orbit-legend-name", "last.fm" }
                    span { class: "orbit-legend-tail", "tag overlap" }
                }
            }

            div { class: "orbit-status",
                div { class: "orbit-status-led" }
                span { "{status_text}" }
            }
        }
    }
}

#[component]
fn DiscoverSeedBanner(
    seed_text: String,
    count: usize,
    sc_count: u32,
    lb_count: u32,
    lf_count: u32,
    sc_enabled: bool,
    lb_enabled: bool,
    lf_enabled: bool,
    lf_configured: bool,
    mode: DiscoveryMode,
    on_reroll: EventHandler<()>,
) -> Element {
    let eyebrow = match mode {
        DiscoveryMode::SimilarTo => "similar to",
        DiscoveryMode::CrossPlatformBridge => "bridge seed",
    };

    rsx! {
        div { class: "discover-seed-banner",
            div { class: "discover-seed-mini", span {} }
            div {
                div { class: "discover-seed-eyebrow", "{eyebrow}" }
                div { class: "discover-seed-text", "{seed_text}" }
                div { class: "discover-seed-stats",
                    span { "{count} candidates · " }
                    if sc_enabled {
                        span { class: "stats-src stats-src-sc", "sc {sc_count}" }
                    } else {
                        span { class: "stats-src stats-src-off", "sc off" }
                    }
                    span { class: "stats-sep", " · " }
                    if lb_enabled {
                        span { class: "stats-src stats-src-lb", "lb {lb_count}" }
                    } else {
                        span { class: "stats-src stats-src-off", "lb off" }
                    }
                    span { class: "stats-sep", " · " }
                    if !lf_enabled {
                        span { class: "stats-src stats-src-off", "lf off" }
                    } else if lf_configured {
                        span { class: "stats-src stats-src-lf", "lf {lf_count}" }
                    } else {
                        span { class: "stats-src stats-src-off",
                            title: "Set a Last.fm API key in Settings to enable this source.",
                            "lf no key"
                        }
                    }
                }
            }
            Button {
                label: "re-run".to_string(),
                icon: Some("fa-solid fa-rotate".to_string()),
                variant: ButtonVariant::Ghost,
                size: ButtonSize::Sm,
                on_click: move |_| on_reroll.call(()),
            }
        }
    }
}

/// `DiscoveryResult.rationale` is formatted as
/// `"<sources joined by + > · score <X.XX>"` by the discovery engine,
/// e.g. `"SoundCloud + ListenBrainz · score 0.81"`. We parse it back
/// out here so the row can render filled / unfilled source dots —
/// gives the user a much faster read than the joined string ever did.
fn sources_from_rationale(r: &str) -> (bool, bool, bool) {
    (
        r.contains("SoundCloud"),
        r.contains("ListenBrainz"),
        r.contains("Last.fm"),
    )
}

#[component]
fn DiscoveryRow(result: DiscoveryResult, on_play: EventHandler<()>) -> Element {
    let cover = result.cover_url.clone().unwrap_or_default();
    let score_pct = (result.score * 100.0).round() as i32;
    let rationale = result.rationale.clone();
    let (has_sc_src, has_lb_src, has_lf_src) = sources_from_rationale(&rationale);
    let has_spotify = result.spotify.is_some();
    let has_soundcloud = result.soundcloud.is_some();
    let ctx = use_ctx_menu();
    let ctx_target: Option<Track> = result.play_target();
    // DiscoveryResult only carries the artist string; pick up URIs from
    // a resolved provider track so the row's artist name is clickable.
    let artist_refs = result
        .spotify
        .as_ref()
        .or(result.soundcloud.as_ref())
        .map(|t| t.artists.clone())
        .unwrap_or_default();
    let has_artist_uri = artist_refs.iter().any(|a| !a.uri.0.is_empty());

    rsx! {
        li {
            class: "discovery-row-v2",
            title: "{rationale}",
            onclick: move |_| on_play.call(()),
            oncontextmenu: {
                let ctx_target = ctx_target.clone();
                move |e: Event<MouseData>| {
                    e.prevent_default();
                    let Some(t) = ctx_target.clone() else { return; };
                    open_track_context(ctx, e, t);
                }
            },
            div { class: "track-cover",
                if !cover.is_empty() {
                    img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
                } else {
                    div { class: "track-cover-fallback",
                        i { class: "fa-solid fa-music" }
                    }
                }
            }
            div { class: "track-meta",
                div { class: "track-title", "{result.title}" }
                div { class: "track-artist",
                    if has_artist_uri {
                        ArtistLinks { artists: artist_refs.clone() }
                    } else {
                        "{result.artist}"
                    }
                }
            }
            div { class: "discovery-sources",
                span {
                    class: if has_sc_src { "discovery-source sc on" } else { "discovery-source sc" },
                    title: "SoundCloud related",
                }
                span {
                    class: if has_lb_src { "discovery-source lb on" } else { "discovery-source lb" },
                    title: "ListenBrainz similarity",
                }
                span {
                    class: if has_lf_src { "discovery-source lf on" } else { "discovery-source lf" },
                    title: "Last.fm tag overlap",
                }
            }
            div { class: "discovery-score-v2", "{score_pct}" }
            div { class: "discovery-providers",
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

#[component]
fn BridgeResult(bridge: CrossPlatformMatch) -> Element {
    let source = bridge.source.clone();
    let source_label = source.provider.label();
    let source_cover = source.cover_url.clone().unwrap_or_default();
    let spotify = bridge.spotify.clone();
    let soundcloud = bridge.soundcloud.clone();

    // Target side header: whichever provider(s) we resolved against
    // (not the seed's own). If we resolved to both, show "SP · SC".
    let target_label = match (spotify.as_ref(), soundcloud.as_ref()) {
        (Some(_), Some(_)) => "spotify · soundcloud".to_string(),
        (Some(_), None) => "spotify".to_string(),
        (None, Some(_)) => "soundcloud".to_string(),
        (None, None) => "no match".to_string(),
    };
    let target_provider_attr = match (spotify.as_ref(), soundcloud.as_ref()) {
        (Some(_), Some(_)) => "Spotify",
        (Some(_), None) => "Spotify",
        (None, Some(_)) => "SoundCloud",
        (None, None) => "Local",
    };

    rsx! {
        div { class: "discover-bridge",
            div { class: "bridge-side bridge-source", "data-provider": "{source_label}",
                div { class: "bridge-side-eyebrow",
                    "from "
                    span { class: "bridge-provider-mark", "{source_label}" }
                }
                div { class: "bridge-track-card",
                    div { class: "track-cover",
                        if !source_cover.is_empty() {
                            img { src: "{source_cover}", alt: "", loading: "lazy", decoding: "async" }
                        } else {
                            div { class: "track-cover-fallback",
                                i { class: "fa-solid fa-music" }
                            }
                        }
                    }
                    div { class: "track-meta",
                        div { class: "track-title", "{source.title}" }
                        div { class: "track-artist",
                            ArtistLinks { artists: source.artists.clone() }
                        }
                    }
                }
            }

            div { class: "bridge-connector",
                div { class: "bridge-connector-glyph",
                    i { class: "fa-solid fa-arrow-right-arrow-left" }
                }
            }

            div { class: "bridge-side bridge-target", "data-provider": "{target_provider_attr}",
                div { class: "bridge-side-eyebrow",
                    "to "
                    span { class: "bridge-provider-mark", "{target_label}" }
                }
                if spotify.is_none() && soundcloud.is_none() {
                    div { class: "bridge-empty", "no match on another provider" }
                }
                if let Some(sp) = spotify.as_ref() {
                    BridgeTargetCard {
                        track: sp.clone(),
                        badge_class: "spotify".to_string(),
                        badge_label: "S".to_string(),
                    }
                }
                if let Some(sc) = soundcloud.as_ref() {
                    BridgeTargetCard {
                        track: sc.clone(),
                        badge_class: "soundcloud".to_string(),
                        badge_label: "SC".to_string(),
                    }
                }
            }
        }
    }
}

#[component]
fn BridgeTargetCard(track: Track, badge_class: String, badge_label: String) -> Element {
    let cover = track.cover_url.clone().unwrap_or_default();
    let badge_class_inline = format!("track-badge {badge_class}");

    rsx! {
        PlayableButton {
            track: track.clone(),
            tracks: TrackCtx::new(vec![track.clone()]),
            index: 0,
            class: "bridge-target-btn".to_string(),
            div { class: "bridge-track-card",
                div { class: "track-cover",
                    if !cover.is_empty() {
                        img { src: "{cover}", alt: "", loading: "lazy", decoding: "async" }
                    } else {
                        div { class: "track-cover-fallback",
                            i { class: "fa-solid fa-music" }
                        }
                    }
                }
                div { class: "track-meta",
                    div { class: "track-title", "{track.title}" }
                    div { class: "track-artist",
                        ArtistLinks { artists: track.artists.clone() }
                    }
                }
                span { class: "{badge_class_inline}", "{badge_label}" }
            }
        }
    }
}
