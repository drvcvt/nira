// Thin shell. Owns nothing beyond the active section signal — every domain
// (audio, discovery, library, search, settings) lives in its own crate under
// `hooks/` (state) and `pages/` (rendering), so a change to one doesn't
// force the whole app to re-render.

use std::sync::Arc;

use components::Section;
use config::AppConfig;
use dioxus::prelude::*;
use hooks::{AppContext, DetailView, Player, use_detail};
use provider_hires-provider::the hi-res providerProvider;
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

#[cfg(target_os = "linux")]
mod mpris_bridge;

// Bundled text fonts. Declared via @font-face below instead of relying on
// fontconfig: the system has Bitstream Charter only as bitmap PCF (pixelated
// at heading sizes) and webkit2gtk picks odd instances out of installed
// variable fonts. Bundling pins exact font data on every machine.
const FONT_GEIST: Asset = asset!("../assets/fonts/Geist-Variable.ttf");
const FONT_GEIST_ITALIC: Asset = asset!("../assets/fonts/Geist-Italic-Variable.ttf");
const FONT_GEIST_MONO: Asset = asset!("../assets/fonts/GeistMono-Variable.ttf");
const FONT_GEIST_MONO_ITALIC: Asset = asset!("../assets/fonts/GeistMono-Italic-Variable.ttf");
const FONT_CHARTER: Asset = asset!("../assets/fonts/Charter-Regular.ttf");
const FONT_CHARTER_BOLD: Asset = asset!("../assets/fonts/Charter-Bold.ttf");
// FontAwesome icon fonts, vendored — previously CDN-loaded, which meant a
// visible icon pop-in on every boot and no icons at all offline.
const FONT_FA_SOLID: Asset = asset!("../assets/fonts/fa-solid-900.woff2");
const FONT_FA_REGULAR: Asset = asset!("../assets/fonts/fa-regular-400.woff2");
const FONT_FA_BRANDS: Asset = asset!("../assets/fonts/fa-brands-400.woff2");

// All app CSS inlined into one <style> tag that ships with the very first
// DOM patch. As separate <link> stylesheets the webview painted the raw DOM
// for ~half a second before the 13 css fetches landed (startup FOUC).
// ORDER MATTERS — it reproduces the cascade of the old link order (base
// first, page-specific later, responsive overrides last). Add new page
// styles as a new file + entry here.
const CSS_ALL: &str = concat!(
    include_str!("../assets/css/base.css"),
    include_str!("../assets/css/sidebar.css"),
    include_str!("../assets/css/player.css"),
    include_str!("../assets/css/search.css"),
    include_str!("../assets/css/tracks.css"),
    include_str!("../assets/css/settings.css"),
    include_str!("../assets/css/home.css"),
    include_str!("../assets/css/discover.css"),
    include_str!("../assets/css/menu.css"),
    include_str!("../assets/css/detail.css"),
    include_str!("../assets/css/library.css"),
    include_str!("../assets/css/buttons.css"),
    include_str!("../assets/css/responsive.css"),
);
// FontAwesome utility classes (6.5.1, @font-face blocks stripped at vendor
// time — the faces are declared below against the bundled woff2s instead).
const CSS_FONTAWESOME: &str = include_str!("../assets/fontawesome/fontawesome.css");

fn main() {
    // The dependency graph enables BOTH rustls 0.23 crypto providers
    // (librespot pulls aws-lc-rs, other deps pull ring). With two providers
    // compiled in, rustls refuses to auto-pick and PANICS at the first TLS
    // handshake — which is the librespot connect, i.e. the first Spotify
    // play. Pin the process default before anything can dial out.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("rustls crypto provider install (first call in main)");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Loud panic hook — Dioxus' desktop runtime sometimes swallows panics
    // from spawned tasks (the webview keeps the main thread alive, but the
    // panicking task's stderr trace can get lost). Force a backtrace to
    // tracing *and* stderr so we don't have to guess where things died.
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("\n=== nira panic ===\n{info}\n{bt}\n==================");
        tracing::error!(%info, "nira panic\n{bt}");
    }));

    // Pre-paint window background — the webview clears to white before the
    // first HTML paint, which reads as a hard flash in dark mode. Pick the
    // theme's canvas colour before the window ever maps. System preference
    // maps to dark: a dark flash under a light theme is gentler than a
    // white flash under a dark one.
    let bg = match AppConfig::load().unwrap_or_default().theme {
        config::ThemePref::Light => (250, 250, 250, 255),
        _ => (18, 18, 18, 255),
    };

    let cfg = dioxus::desktop::Config::new()
        .with_background_color(bg)
        // Drop tao's "Window / Edit / Help" default menu bar — it sits below
        // the OS titlebar and clashes with the app's own chrome.
        .with_menu(None)
        .with_window(
            dioxus::desktop::WindowBuilder::new()
                .with_title("nira")
                .with_resizable(true)
                .with_decorations(false)
                .with_inner_size(dioxus::desktop::tao::dpi::LogicalSize::new(1280.0, 800.0)),
        );

    dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(App);
}

#[component]
fn App() -> Element {
    // Boot-time singletons. The construction itself is cheap and synchronous
    // — Player::spawn is the slowest at ~10-20 ms (it blocks on cpal handing
    // back the device) but everything else is just Arc allocation. We then
    // *do* fire one background task to pre-warm the SC client_id so the
    // first user-visible search doesn't pay the JS-scrape cost.
    let app_cfg = use_hook(|| AppConfig::load().unwrap_or_default());
    let player = use_hook({
        let app_cfg = app_cfg.clone();
        move || {
            Player::spawn(AppConfig::play_history_path(), app_cfg.volume)
                .expect("audio engine failed to start")
        }
    });
    let sc = use_hook(|| Arc::new(SoundCloudProvider::new().expect("SoundCloud provider init")));
    let sp = use_hook(|| {
        let client_id = app_cfg.spotify_client_id.clone().unwrap_or_default();
        let tokens_path = AppConfig::spotify_tokens_path();
        Arc::new(SpotifyProvider::new(client_id, tokens_path).expect("Spotify provider init"))
    });
    let qz = use_hook(|| {
        Arc::new(
            the hi-res providerProvider::new(app_cfg.hires-provider_format_id, app_cfg.hires-provider_token.clone())
                .expect("the hi-res provider provider init"),
        )
    });
    AppContext::install(player.clone(), sc.clone(), sp, qz, app_cfg);

    // Background prewarm — SC needs a client_id from the public web player.
    // Doing it now means the first search/Discovery call is ~1 s faster.
    use_hook(|| {
        let sc = sc.clone();
        spawn(async move {
            sc.prewarm().await;
        });
    });

    // MPRIS — Linux media keys + KDE/GNOME now-playing widgets. Runs in
    // its own thread with its own runtime since zbus uses async-io by
    // default. No-op on non-Linux.
    #[cfg(target_os = "linux")]
    use_hook(|| {
        mpris_bridge::start(player.clone());
    });

    // Serve locally-extracted album art to the webview: the library scanner
    // writes covers into the cache dir, tracks carry "/covers/<file>" URLs,
    // and this handler answers those requests with the image bytes.
    {
        let covers_dir = AppConfig::covers_cache_dir();
        dioxus::desktop::use_asset_handler("covers", move |request, responder| {
            let not_found = || {
                dioxus::desktop::wry::http::Response::builder()
                    .status(404)
                    .body(Vec::new())
                    .unwrap_or_default()
            };
            // Last path segment only — no separators survive, so a crafted
            // URL can't traverse out of the covers directory.
            let name = request
                .uri()
                .path()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            let Some(dir) = covers_dir.as_ref() else {
                responder.respond(not_found());
                return;
            };
            if name.is_empty() || name.contains("..") {
                responder.respond(not_found());
                return;
            }
            match std::fs::read(dir.join(&name)) {
                Ok(bytes) => {
                    let mime = if name.ends_with(".png") {
                        "image/png"
                    } else {
                        "image/jpeg"
                    };
                    let resp = dioxus::desktop::wry::http::Response::builder()
                        .header("Content-Type", mime)
                        .body(bytes)
                        .unwrap_or_default();
                    responder.respond(resp);
                }
                Err(_) => responder.respond(not_found()),
            }
        });
    }

    let section = use_signal(|| Section::Home);
    let search_open = use_signal(|| false);
    // For the shell-level Escape handler — the menu itself has no focus, so
    // its dismiss key has to live where keydown events actually land.
    let ctx_menu = hooks::use_ctx_menu();

    // Appearance → document root. Theme sets `data-theme` (System = no
    // attribute so the CSS prefers-color-scheme default applies); the UI
    // font sets the `--font-ui` variable. Runs once on boot and again
    // whenever Settings flips the config signal.
    let config_sig = hooks::use_config();
    use_effect(move || {
        let cfg = config_sig.read();
        let theme_js = match cfg.theme {
            hooks::ThemePref::Light => {
                "document.documentElement.setAttribute('data-theme','light');"
            }
            hooks::ThemePref::Dark => {
                "document.documentElement.setAttribute('data-theme','dark');"
            }
            hooks::ThemePref::System => "document.documentElement.removeAttribute('data-theme');",
        };
        let font_stack = hooks::ui_font_stack(cfg.ui_font.as_deref());
        let js = format!(
            "{theme_js}document.documentElement.style.setProperty('--font-ui', '{font_stack}');"
        );
        document::eval(&js);
    });

    rsx! {
        document::Style {
            {format!(
                r#"
@font-face {{ font-family: "Geist"; src: url("{FONT_GEIST}") format("truetype"); font-weight: 100 900; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Geist"; src: url("{FONT_GEIST_ITALIC}") format("truetype"); font-weight: 100 900; font-style: italic; font-display: block; }}
@font-face {{ font-family: "Geist Mono"; src: url("{FONT_GEIST_MONO}") format("truetype"); font-weight: 100 900; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Geist Mono"; src: url("{FONT_GEIST_MONO_ITALIC}") format("truetype"); font-weight: 100 900; font-style: italic; font-display: block; }}
@font-face {{ font-family: "Charter"; src: url("{FONT_CHARTER}") format("truetype"); font-weight: 400; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Charter"; src: url("{FONT_CHARTER_BOLD}") format("truetype"); font-weight: 700; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Font Awesome 6 Free"; src: url("{FONT_FA_SOLID}") format("woff2"); font-weight: 900; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Font Awesome 6 Free"; src: url("{FONT_FA_REGULAR}") format("woff2"); font-weight: 400; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Font Awesome 6 Brands"; src: url("{FONT_FA_BRANDS}") format("woff2"); font-weight: 400; font-style: normal; font-display: block; }}
"#
            )}
        }
        document::Style { {CSS_ALL} }
        document::Style { {CSS_FONTAWESOME} }
        document::Script {
            "(function(){{\
                if (!window.__nira_hotkeys_installed) {{\
                    window.__nira_hotkeys_installed = true;\
                    window.__nira_focus_search = function() {{\
                        var tries = 0;\
                        function focusWhenReady() {{\
                            var input = document.querySelector('.search-overlay.open .searchbar-input');\
                            if (input) {{\
                                input.focus({{ preventScroll: true }});\
                                input.select && input.select();\
                                return;\
                            }}\
                            if (++tries < 12) requestAnimationFrame(focusWhenReady);\
                        }}\
                        requestAnimationFrame(focusWhenReady);\
                    }};\
                    document.addEventListener('keydown', function(e) {{\
                        var key = (e.key || '').toLowerCase();\
                        var isSpace = key === ' ' || key === 'space' || key === 'spacebar';\
                        var openSearch = ((e.ctrlKey || e.metaKey) && key === 'f') || (e.altKey && isSpace);\
                        if (openSearch) {{\
                            e.preventDefault();\
                            var open = document.getElementById('nira-search-hotkey');\
                            if (open) open.click();\
                            window.__nira_focus_search();\
                            return;\
                        }}\
                        if (key === 'escape' && document.querySelector('.search-overlay.open')) {{\
                            e.preventDefault();\
                            var close = document.getElementById('nira-search-close-hotkey');\
                            if (close) close.click();\
                        }}\
                    }}, true);\
                }}\
            }})();"
        }
        div {
            class: if *search_open.read() { "shell search-open" } else { "shell" },
            tabindex: "0",
            // Suppress wry's native context menu app-wide — without this
            // the webview overlays its own menu on top of ours. Track-row
            // handlers still fire and call ctx.open() because their
            // preventDefault doesn't stop propagation.
            onmounted: move |e: Event<MountedData>| {
                spawn(async move {
                    let _ = e.data.set_focus(true).await;
                });
            },
            oncontextmenu: move |e: Event<MouseData>| {
                e.prevent_default();
            },
            onkeydown: {
                let mut search_open = search_open;
                move |e: Event<KeyboardData>| {
                    let mods = e.modifiers();
                    let key = e.key().to_string();
                    let ctrl_or_meta = mods.contains(Modifiers::CONTROL) || mods.contains(Modifiers::META);
                    let is_space = key == " " || key.eq_ignore_ascii_case("space");
                    let open_search = (ctrl_or_meta && key.eq_ignore_ascii_case("f"))
                        || (mods.contains(Modifiers::ALT) && is_space);
                    if open_search {
                        e.prevent_default();
                        search_open.set(true);
                    } else if e.key() == Key::Escape && ctx_menu.current.peek().is_some() {
                        e.prevent_default();
                        ctx_menu.close();
                    } else if e.key() == Key::Escape && *search_open.peek() {
                        e.prevent_default();
                        search_open.set(false);
                    }
                }
            },
            button {
                id: "nira-search-hotkey",
                class: "hotkey-bridge",
                r#type: "button",
                tabindex: "-1",
                onclick: {
                    let mut search_open = search_open;
                    move |_| search_open.set(true)
                },
            }
            button {
                id: "nira-search-close-hotkey",
                class: "hotkey-bridge",
                r#type: "button",
                tabindex: "-1",
                onclick: {
                    let mut search_open = search_open;
                    move |_| search_open.set(false)
                },
            }
            // Visible search trigger for mouse users; Ctrl+F / Alt+Space
            // keep working via the hotkey bridges above.
            button {
                class: "corner-search",
                title: "Search (Ctrl+F)",
                onclick: {
                    let mut search_open = search_open;
                    move |_| {
                        search_open.set(true);
                        document::eval(
                            "window.__nira_focus_search && window.__nira_focus_search();",
                        );
                    }
                },
                i { class: "fa-solid fa-magnifying-glass" }
            }
            components::sidebar::Sidebar { section }
            main { class: "content",
                MainContent { section }
            }
            components::bottombar::Bottombar {}
            pages::search_overlay::SearchOverlay { open: search_open }
            // Global right-click menu — singleton, reads its own state from
            // the `use_ctx_menu` signal. Rows on any page open it.
            components::ctx_menu::ContextMenu {}
            // Bottom-left toast for the hi-res provider download progress/result.
            components::download_toast::DownloadToast {}
        }
    }
}

/// Splits the section-vs-detail rendering out so the `use_detail` hook
/// runs inside a component (rather than the App root, where it would
/// create a context cycle).
#[component]
fn MainContent(section: Signal<Section>) -> Element {
    let detail = use_detail();
    let detail_views = detail.views();
    let detail_open = !detail_views.is_empty();
    let top_idx = detail_views.len().saturating_sub(1);
    rsx! {
        // The active section stays mounted (display:none) under any detail
        // pages so its state — Library tab, pagination, Discover results —
        // survives opening an artist/album and coming back. display:contents
        // keeps the visible layout identical to rendering the page bare.
        div {
            style: if detail_open { "display:none;" } else { "display:contents;" },
            {match *section.read() {
                Section::Home     => rsx! { pages::home::Home {} },
                Section::Discover => rsx! { pages::discover::Discover {} },
                Section::Library  => rsx! { pages::library::Library {} },
                Section::Settings => rsx! { pages::settings::Settings {} },
            }}
        }
        // Same trick for the detail stack itself: every level stays mounted,
        // only the top is visible. Keyed by position + URI so artist → album
        // → Back returns to the artist page exactly as it was left (active
        // tab, loaded data) instead of remounting and refetching it.
        for (idx, d) in detail_views.iter().enumerate() {
            div {
                key: "{idx}-{detail_view_key(d)}",
                style: if idx == top_idx { "display:contents;" } else { "display:none;" },
                {match d.clone() {
                    DetailView::Artist(uri) => rsx! { pages::artist::ArtistPage { uri } },
                    DetailView::Album(uri)  => rsx! { pages::album::AlbumPage { uri } },
                }}
            }
        }
    }
}

fn detail_view_key(d: &DetailView) -> &str {
    match d {
        DetailView::Artist(uri) => &uri.0,
        DetailView::Album(uri) => &uri.0,
    }
}
