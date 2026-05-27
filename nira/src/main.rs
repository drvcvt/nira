// Thin shell. Owns nothing beyond the active section signal — every domain
// (audio, discovery, library, search, settings) lives in its own crate under
// `hooks/` (state) and `pages/` (rendering), so a change to one doesn't
// force the whole app to re-render.

use std::sync::Arc;

use components::Section;
use config::AppConfig;
use dioxus::prelude::*;
use hooks::{AppContext, DetailView, Player, use_detail};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

#[cfg(target_os = "linux")]
mod mpris_bridge;

const MAIN_CSS: Asset = asset!("../assets/main.css");

fn main() {
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

    let cfg = dioxus::desktop::Config::new()
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
    AppContext::install(player.clone(), sc.clone(), sp, app_cfg);

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

    let section = use_signal(|| Section::Home);
    let search_open = use_signal(|| false);

    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        // CDN-load JetBrains Mono + FontAwesome via inline script (Dioxus's
        // `asset!` doesn't take remote URLs, and bundling a webfont would
        // bloat the binary). Mirrors kopuz's approach for parity of feel.
        document::Script {
            "(function(){{\
                ['https://fonts.bunny.net/css?family=jetbrains-mono:400,500,600,700&display=swap',\
                 'https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.5.1/css/all.min.css']\
                .forEach(function(href){{\
                    var l=document.createElement('link');\
                    l.rel='stylesheet';l.href=href;\
                    document.head.appendChild(l);\
                }});\
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
            components::sidebar::Sidebar { section }
            main { class: "content",
                MainContent { section }
            }
            components::bottombar::Bottombar {}
            pages::search_overlay::SearchOverlay { open: search_open }
            // Global right-click menu — singleton, reads its own state from
            // the `use_ctx_menu` signal. Rows on any page open it.
            components::ctx_menu::ContextMenu {}
        }
    }
}

/// Splits the section-vs-detail rendering out so the `use_detail` hook
/// runs inside a component (rather than the App root, where it would
/// create a context cycle).
#[component]
fn MainContent(section: Signal<Section>) -> Element {
    let detail = use_detail();
    let current_detail = detail.current.read().clone();
    rsx! {
        if let Some(d) = current_detail {
            {match d {
                DetailView::Artist(uri) => rsx! { pages::artist::ArtistPage { uri } },
                DetailView::Album(uri)  => rsx! { pages::album::AlbumPage { uri } },
            }}
        } else {
            {match *section.read() {
                Section::Home     => rsx! { pages::home::Home {} },
                Section::Discover => rsx! { pages::discover::Discover {} },
                Section::Library  => rsx! { pages::library::Library {} },
                Section::Settings => rsx! { pages::settings::Settings {} },
            }}
        }
    }
}
