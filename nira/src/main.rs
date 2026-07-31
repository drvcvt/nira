// Thin shell. Owns nothing beyond the active section signal — every domain
// (audio, discovery, library, search, settings) lives in its own crate under
// `hooks/` (state) and `pages/` (rendering), so a change to one doesn't
// force the whole app to re-render.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use components::Section;
use config::AppConfig;
use dioxus::prelude::*;
use hooks::{AppContext, DetailView, Player, use_detail};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

mod discord_bridge;

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

// All app CSS inlined into the initial document head. Dioxus' runtime head
// insertion uses a queued effect after the body patch, which lets WebKit
// paint the raw DOM first (startup FOUC).
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
    include_str!("../assets/css/binds.css"),
    include_str!("../assets/css/viz.css"),
    include_str!("../assets/css/cover.css"),
    include_str!("../assets/css/responsive.css"),
);
// FontAwesome utility classes (6.5.1, @font-face blocks stripped at vendor
// time — the faces are declared below against the bundled woff2s instead).
const CSS_FONTAWESOME: &str = include_str!("../assets/fontawesome/fontawesome.css");

fn initial_head() -> String {
    format!(
        r#"<style>
@font-face {{ font-family: "Geist"; src: url("{FONT_GEIST}") format("truetype"); font-weight: 100 900; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Geist"; src: url("{FONT_GEIST_ITALIC}") format("truetype"); font-weight: 100 900; font-style: italic; font-display: block; }}
@font-face {{ font-family: "Geist Mono"; src: url("{FONT_GEIST_MONO}") format("truetype"); font-weight: 100 900; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Geist Mono"; src: url("{FONT_GEIST_MONO_ITALIC}") format("truetype"); font-weight: 100 900; font-style: italic; font-display: block; }}
@font-face {{ font-family: "Charter"; src: url("{FONT_CHARTER}") format("truetype"); font-weight: 400; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Charter"; src: url("{FONT_CHARTER_BOLD}") format("truetype"); font-weight: 700; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Font Awesome 6 Free"; src: url("{FONT_FA_SOLID}") format("woff2"); font-weight: 900; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Font Awesome 6 Free"; src: url("{FONT_FA_REGULAR}") format("woff2"); font-weight: 400; font-style: normal; font-display: block; }}
@font-face {{ font-family: "Font Awesome 6 Brands"; src: url("{FONT_FA_BRANDS}") format("woff2"); font-weight: 400; font-style: normal; font-display: block; }}
{CSS_ALL}
{CSS_FONTAWESOME}
</style>"#
    )
}

fn main() {
    // The dependency graph enables BOTH rustls 0.23 crypto providers
    // (librespot pulls aws-lc-rs, other deps pull ring). With two providers
    // compiled in, rustls refuses to auto-pick and PANICS at the first TLS
    // handshake — which is the librespot connect, i.e. the first Spotify
    // play. Pin the process default before anything can dial out.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("rustls crypto provider install (first call in main)");

    // Instance lock BEFORE the log sink: a second instance must exit
    // without truncating the running instance's nira.log. Failures here go
    // to stderr — tracing isn't up yet, and these paths are terminal.
    let Some(lock_path) = AppConfig::cache_dir().map(|dir| dir.join("instance.lock")) else {
        eprintln!("nira: could not resolve cache directory for instance lock");
        return;
    };
    if let Some(parent) = lock_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("nira: could not create instance-lock directory: {error}");
        return;
    }
    let _instance_lock = match acquire_instance_lock(&lock_path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            eprintln!("nira: already running");
            return;
        }
        Err(error) => {
            eprintln!("nira: could not acquire instance lock: {error}");
            return;
        }
    };

    // Log to stderr AND cache/nira.log (fresh file per boot). The launcher
    // starts nira detached, so without the file sink a daily-driven session
    // has no logs to diagnose slow loads or errors after the fact.
    let log_file = AppConfig::cache_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::File::create(dir.join("nira.log")).ok()
    });
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(move || TeeWriter {
            file: log_file.as_ref().and_then(|f| f.try_clone().ok()),
        })
        .init();

    // Crash hygiene: drop atomic-write temp files a previous instance left
    // behind (its rename never happened; they're garbage forever otherwise).
    config::sweep_stale_tmp_files();

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
        .with_custom_head(initial_head())
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

fn acquire_instance_lock(path: &Path) -> std::io::Result<File> {
    let file = File::options().create(true).write(true).open(path)?;
    file.try_lock()?;
    Ok(file)
}

/// Duplicates tracing output to stderr and the boot's log file.
struct TeeWriter {
    file: Option<File>,
}

impl std::io::Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(f) = self.file.as_mut() {
            let _ = f.write_all(buf);
        }
        std::io::stderr().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = self.file.as_mut() {
            let _ = f.flush();
        }
        std::io::stderr().flush()
    }
}

#[component]
fn AudioStartupFailure(error: String) -> Element {
    rsx! {
        main { class: "audio-startup-failure",
            section { class: "audio-startup-failure-copy",
                h1 { "Audio output could not start" }
                p { class: "audio-startup-failure-error", "{error}" }
                p { "The output device may be missing or busy." }
                p { "Check the system audio output, close any app holding it exclusively, then restart Nira." }
            }
        }
    }
}

#[component]
fn App() -> Element {
    // Boot-time singletons. The construction itself is cheap and synchronous
    // — Player::spawn is the slowest at ~10-20 ms (it blocks on cpal handing
    // back the device) but everything else is just Arc allocation. We then
    // *do* fire one background task to pre-warm the SC client_id so the
    // first user-visible search doesn't pay the JS-scrape cost.
    let app_cfg = use_hook(|| AppConfig::load().unwrap_or_default());
    let player_result = use_hook({
        let app_cfg = app_cfg.clone();
        move || {
            Player::spawn(AppConfig::play_history_path(), app_cfg.volume)
                .map_err(|error| error.to_string())
        }
    });
    let player = match &player_result {
        Ok(player) => player.clone(),
        Err(error) => return rsx! { AudioStartupFailure { error: error.to_string() } },
    };
    let sc = use_hook(|| Arc::new(SoundCloudProvider::new().expect("SoundCloud provider init")));
    let sp = use_hook(|| {
        let client_id = app_cfg.spotify_client_id.clone().unwrap_or_default();
        let tokens_path = AppConfig::spotify_tokens_path();
        Arc::new(SpotifyProvider::new(client_id, tokens_path).expect("Spotify provider init"))
    });
    AppContext::install(player.clone(), sc.clone(), sp, app_cfg);
    let enrichment = hooks::use_enrichment();
    let config_sig = hooks::use_config();
    let discord_presence_enabled =
        use_hook(|| Arc::new(AtomicBool::new(config_sig.read().discord_presence)));
    use_effect({
        let enabled = discord_presence_enabled.clone();
        move || enabled.store(config_sig.read().discord_presence, Ordering::Relaxed)
    });

    // Visualizer open-state — shared by the bottombar button, the V-key
    // bridge and the overlay itself.
    let viz_open = use_signal(|| false);
    use_context_provider(|| components::visualizer::VizOpen(viz_open));

    // Fullscreen cover overlay open-state — shared by the bottombar's mini
    // cover, the Escape bridge and the overlay itself.
    let cover_open = use_signal(|| false);
    use_context_provider(|| components::cover::CoverOpen(cover_open));

    // Pre-mute volume stash — shared by the M-key bridge and the
    // bottombar's speaker button.
    let mute_stash = use_signal(|| None::<f32>);
    use_context_provider(|| components::hotkeys::MuteStash(mute_stash));

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

    // Discord Rich Presence is local IPC only and provider-blind. The bridge
    // owns its reconnect loop so Discord can start before or after Nira.
    use_hook({
        let enabled = discord_presence_enabled.clone();
        move || discord_bridge::start(player.clone(), enabled, enrichment.clone())
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
    // Last appearance actually pushed to the DOM. The config signal is also
    // written by `set_volume`, so without this guard a volume drag fired one
    // `document::eval` round-trip per slider step — each one rewriting
    // `--font-ui` on the root element, which invalidates style for everything
    // that inherits it, on the thread servicing the drag.
    let mut applied_appearance = use_signal(|| None::<(hooks::ThemePref, &'static str)>);
    use_effect(move || {
        let cfg = config_sig.read();
        let font_stack = hooks::ui_font_stack(cfg.ui_font.as_deref());
        if *applied_appearance.peek() == Some((cfg.theme, font_stack)) {
            return;
        }
        applied_appearance.set(Some((cfg.theme, font_stack)));
        let theme_js = match cfg.theme {
            hooks::ThemePref::Light => {
                "document.documentElement.setAttribute('data-theme','light');"
            }
            hooks::ThemePref::Dark => {
                "document.documentElement.setAttribute('data-theme','dark');"
            }
            hooks::ThemePref::System => "document.documentElement.removeAttribute('data-theme');",
        };
        let js = format!(
            "{theme_js}document.documentElement.style.setProperty('--font-ui', '{font_stack}');"
        );
        document::eval(&js);
    });

    rsx! {
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
                    var press = function(id) {{\
                        var el = document.getElementById(id);\
                        if (el) el.click();\
                    }};\
                    var typing = function(e) {{\
                        var t = e.target;\
                        return !!(t && t.closest && t.closest('input, textarea, select, [contenteditable=\"true\"]'));\
                    }};\
                    document.addEventListener('keydown', function(e) {{\
                        if (document.querySelector('.ctx-menu')) return;\
                        var key = (e.key || '').toLowerCase();\
                        var isSpace = key === ' ' || key === 'space' || key === 'spacebar';\
                        var mod = e.ctrlKey || e.metaKey;\
                        var openSearch = (mod && key === 'f') || (e.altKey && isSpace);\
                        if (openSearch) {{\
                            e.preventDefault();\
                            press('nira-search-hotkey');\
                            window.__nira_focus_search();\
                            return;\
                        }}\
                        if (mod && key === '/') {{\
                            e.preventDefault();\
                            press('nira-key-binds');\
                            return;\
                        }}\
                        if (key === 'escape') {{\
                            /* stopPropagation as well as preventDefault: this\
                               is a capture listener, so without it the same\
                               keydown still reaches the search overlay's own\
                               onkeydown and the shell handler, and one Escape\
                               closes two overlays at once. */\
                            var dismiss = function(id) {{\
                                e.preventDefault();\
                                e.stopPropagation();\
                                press(id);\
                            }};\
                            if (document.querySelector('.binds-overlay.open')) {{\
                                dismiss('nira-key-binds-close');\
                                return;\
                            }}\
                            if (document.querySelector('.viz-overlay')) {{\
                                dismiss('nira-key-viz-close');\
                                return;\
                            }}\
                            /* :not(.closing) — the element lingers for the\
                               400ms close animation, and matching it there\
                               swallowed the next Escape. */\
                            if (document.querySelector('.cover-overlay:not(.closing)')) {{\
                                dismiss('nira-key-cover-close');\
                                return;\
                            }}\
                            if (document.querySelector('.search-overlay.open')) {{\
                                dismiss('nira-search-close-hotkey');\
                                return;\
                            }}\
                            if (document.querySelector('.queue-popover')) {{\
                                dismiss('nira-key-queue-close');\
                            }}\
                            return;\
                        }}\
                        if (typing(e)) return;\
                        /* Space must still activate a focused button/row. The\
                           typing() guard above only exempts text inputs, and\
                           this listener stopPropagation()s, so without this\
                           no button in the app responded to Space. */\
                        if (isSpace &&\
                            e.target &&\
                            e.target.closest &&\
                            e.target.closest('button:not(.hotkey-bridge), [role=button]')\
                        ) return;\
                        var acted = false;\
                        if (mod && !e.altKey && !e.shiftKey) {{\
                            if (key === 'arrowright') {{ press('nira-key-next'); acted = true; }}\
                            else if (key === 'arrowleft') {{ press('nira-key-prev'); acted = true; }}\
                            else if (key === 'arrowup') {{ press('nira-key-volup'); acted = true; }}\
                            else if (key === 'arrowdown') {{ press('nira-key-voldown'); acted = true; }}\
                        }} else if (!mod && !e.altKey && e.shiftKey) {{\
                            if (key === 'arrowright') {{ press('nira-key-seek-fwd'); acted = true; }}\
                            else if (key === 'arrowleft') {{ press('nira-key-seek-back'); acted = true; }}\
                        }} else if (!mod && !e.altKey && !e.shiftKey) {{\
                            if (isSpace) {{ press('nira-key-playpause'); acted = true; }}\
                            else if (key === 's') {{ press('nira-key-shuffle'); acted = true; }}\
                            else if (key === 'r') {{ press('nira-key-repeat'); acted = true; }}\
                            else if (key === 'l') {{ press('nira-key-like'); acted = true; }}\
                            else if (key === 'v') {{ press('nira-key-viz'); acted = true; }}\
                            else if (key === 'm') {{ press('nira-key-mute'); acted = true; }}\
                            /* Bare arrows deliberately unbound: .content is\
                               the app's only scroll container, and claiming\
                               them here left the app unscrollable by keyboard.\
                               Ctrl+Up/Down above still adjusts volume. */\
                        }}\
                        if (acted) {{\
                            e.preventDefault();\
                            e.stopPropagation();\
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
                        // Dismiss the ctx menu first — it renders above the
                        // search overlay and its full-screen catcher eats
                        // every click on it, so opening search underneath an
                        // open menu produced an unreachable overlay.
                        if ctx_menu.current.peek().is_some() {
                            ctx_menu.close();
                        }
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
                "aria-label": "Search",
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
            // Media-key bridges + the Ctrl+/ shortcut sheet.
            components::hotkeys::Hotkeys {}
            // Fullscreen audio visualizer (V / bottombar wave button).
            components::visualizer::Visualizer {}
            // Fullscreen cover / vinyl overlay (bottombar mini cover).
            components::cover::CoverOverlay {}
            TogetherIndicator {}
            components::download_toast::DownloadToast {}
        }
    }
}

fn together_indicator_copy(is_host: bool, peers: &[String], streaming: bool) -> String {
    if peers.is_empty() {
        return if is_host {
            "Session ready".into()
        } else {
            "Connecting…".into()
        };
    }

    let state = if streaming { "Streaming" } else { "Connected" };
    if is_host && peers.len() > 1 {
        format!("{state} with {} people · {}", peers.len(), peers.join(", "))
    } else {
        format!("{state} with {}", peers.join(", "))
    }
}

fn guest_sync_label(
    has_source: bool,
    loading: bool,
    unavailable: bool,
    gap_ms: Option<u64>,
) -> &'static str {
    if unavailable {
        "Not in sync"
    } else if loading || !has_source || gap_ms.is_none_or(|gap| gap > 750) {
        "Syncing"
    } else {
        "In sync"
    }
}

#[component]
fn TogetherIndicator() -> Element {
    let together = hooks::use_together();
    let player = hooks::use_player();
    let queue = hooks::use_queue();
    let snapshot = together.snapshot.read().clone();
    let is_host = snapshot.ticket.is_some();
    let connecting = snapshot.status == "connecting…";

    if !is_host && snapshot.peers.is_empty() && !connecting {
        return rsx! {};
    }

    let playback = player.snapshot();
    let (copy, state_class) = if is_host {
        let streaming = playback.has_source && !playback.is_paused;
        (
            together_indicator_copy(true, &snapshot.peers, streaming),
            if streaming { "streaming" } else { "" },
        )
    } else {
        let loading = *queue.is_loading_track.read();
        let current = queue
            .current_index
            .read()
            .and_then(|idx| queue.entries.read().get(idx).cloned());
        let same_track = snapshot.target.as_ref().is_none_or(|target| {
            current.as_ref().is_some_and(|track| {
                track.uri.0 == target.track_uri
                    || (hooks::match_key(&track.title) == hooks::match_key(&target.title)
                        && track.artists.first().is_some_and(|artist| {
                            hooks::match_key(&artist.name) == hooks::match_key(&target.artist)
                        })
                        && track
                            .duration
                            .as_secs()
                            .abs_diff(target.duration_ns / 1_000_000_000)
                            <= 3)
            })
        });
        let unavailable = together.unmatched.read().is_some()
            || queue.error.read().is_some()
            || (playback.has_source && !loading && !same_track);
        let gap_ms = snapshot.target.as_ref().map(|target| {
            let elapsed = if target.playing {
                together.handle().now_ns().saturating_sub(target.at_ns)
            } else {
                0
            };
            let expected = std::time::Duration::from_nanos(target.pos_ns.saturating_add(elapsed));
            playback.position.abs_diff(expected).as_millis() as u64
        });
        let label = if snapshot.stopped && !unavailable {
            "In sync"
        } else {
            guest_sync_label(playback.has_source, loading, unavailable, gap_ms)
        };
        let copy = if snapshot.peers.is_empty() {
            "Connecting…".into()
        } else {
            format!("{label} with {}", snapshot.peers.join(", "))
        };
        let state_class = match label {
            "In sync" => "in-sync",
            "Not in sync" => "out-of-sync",
            _ => "syncing",
        };
        (copy, state_class)
    };
    let class = format!("together-indicator {state_class}");

    rsx! {
        div {
            class,
            role: "status",
            "aria-live": "polite",
            title: "{copy}",
            span { class: "together-indicator-dot", "aria-hidden": "true" }
            span { class: "together-indicator-copy", "{copy}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_sync_label_reports_real_local_health() {
        assert_eq!(guest_sync_label(true, false, false, Some(120)), "In sync");
        assert_eq!(guest_sync_label(true, false, false, Some(900)), "Syncing");
        assert_eq!(guest_sync_label(false, true, false, None), "Syncing");
        assert_eq!(guest_sync_label(true, false, true, Some(0)), "Not in sync");
    }

    #[test]
    fn together_indicator_describes_the_visible_connection() {
        assert_eq!(
            together_indicator_copy(true, &["Alex".into(), "Sam".into()], true),
            "Streaming with 2 people · Alex, Sam"
        );
        assert_eq!(
            together_indicator_copy(false, &["Alex".into()], false),
            "Connected with Alex"
        );
        assert_eq!(together_indicator_copy(true, &[], false), "Session ready");
        assert_eq!(together_indicator_copy(false, &[], false), "Connecting…");
    }

    #[test]
    fn second_instance_lock_is_rejected_until_first_is_dropped() {
        let path = std::env::temp_dir().join(format!(
            "nira-instance-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let first = acquire_instance_lock(&path).unwrap();
        assert!(acquire_instance_lock(&path).is_err());
        drop(first);
        assert!(acquire_instance_lock(&path).is_ok());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn critical_css_is_in_the_initial_document_head() {
        let head = initial_head();
        let source = include_str!("main.rs");
        assert!(head.starts_with("<style>"));
        assert!(head.contains(".shell {"));
        assert!(head.contains(".fa-solid"));
        assert!(head.ends_with("</style>"));
        // Needle built at runtime so this assert's own source line can't
        // satisfy it — only the real builder call site matches.
        let needle = format!(".with_custom_head({}())", "initial_head");
        assert!(source.contains(&needle));
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
