use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    AppConfig, DiscoveryEngine, DiscoverySourcePrefs, use_config, use_discovery_engine,
    use_enrichment, use_player, use_soundcloud, use_spotify,
};

#[component]
pub fn Settings() -> Element {
    rsx! {
        section { class: "page settings-page",
            h1 { "Settings" }
            p { class: "hint", "Audio, library paths, providers, discovery and local data." }

            ConnectionsSettings {}
            LibrarySettings {}
            DiscoverySettings {}
            DataSettings {}
        }
    }
}

#[component]
fn ConnectionsSettings() -> Element {
    let mut config = use_config();
    let sp = use_spotify();
    let sc = use_soundcloud();

    let mut client_id_draft = use_signal({
        let cfg = config.read();
        move || cfg.spotify_client_id.clone().unwrap_or_default()
    });
    let mut spotify_status = use_signal(|| None::<String>);
    let mut is_connecting = use_signal(|| false);

    let initial_token = config.read().listenbrainz_token.clone().unwrap_or_default();
    let initial_user = config
        .read()
        .listenbrainz_username
        .clone()
        .unwrap_or_default();
    let mut lb_token_draft = use_signal(move || initial_token);
    let mut lb_user_draft = use_signal(move || initial_user);
    let mut lb_status = use_signal(|| None::<String>);

    let mut sc_status = use_signal(|| None::<String>);
    let mut sc_refreshing = use_signal(|| false);

    let provider_client_id = sp.client_id();
    let spotify_connected = sp.is_connected();
    let sc_ready = sc.has_cached_client_id();
    let token_active = config
        .read()
        .listenbrainz_token
        .clone()
        .filter(|t| !t.trim().is_empty())
        .is_some();
    let username_active = config
        .read()
        .listenbrainz_username
        .clone()
        .filter(|u| !u.trim().is_empty())
        .is_some();
    let lb_enabled = token_active || username_active;

    rsx! {
        section { class: "settings-group settings-stack",
            h2 { "Connections" }
            p { class: "hint", "Provider credentials and service status." }

            SettingsCard {
                title: "Spotify".to_string(),
                icon: "fa-brands fa-spotify".to_string(),
                status_label: if spotify_connected { "Connected".to_string() } else { "Not connected".to_string() },
                status_ok: spotify_connected,
                p { class: "settings-card-copy",
                    "Bring your own Spotify Developer Client ID. Premium is required for Spotify playback via librespot."
                }
                div { class: "settings-row",
                    label { r#for: "sp-client-id", "Client ID" }
                    input {
                        id: "sp-client-id",
                        r#type: "text",
                        class: "settings-input",
                        placeholder: "32-char hex from your Spotify app",
                        value: "{client_id_draft.read()}",
                        disabled: *is_connecting.read(),
                        oninput: move |e| client_id_draft.set(e.value()),
                    }
                    Button {
                        label: "Save".to_string(),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: *is_connecting.read(),
                        on_click: {
                            let sp = sp.clone();
                            move |_| {
                                let new_id = client_id_draft.read().trim().to_string();
                                let save_result = {
                                    let mut cfg_write = config.write();
                                    cfg_write.spotify_client_id = if new_id.is_empty() { None } else { Some(new_id.clone()) };
                                    cfg_write.save()
                                };
                                match save_result {
                                    Ok(()) => {
                                        spotify_status.set(Some("Saved. Spotify uses this Client ID immediately.".into()));
                                        let sp = sp.clone();
                                        spawn(async move {
                                            match sp.set_client_id(new_id).await {
                                                Ok(true) => spotify_status.set(Some("Client ID changed; old Spotify session was cleared. Connect again if needed.".into())),
                                                Ok(false) => {},
                                                Err(e) => spotify_status.set(Some(format!("Provider update failed: {e}"))),
                                            }
                                        });
                                    }
                                    Err(e) => spotify_status.set(Some(format!("Save failed: {e}"))),
                                }
                            }
                        },
                    }
                }
                div { class: "settings-row compact",
                    label { "Redirect URI" }
                    code { class: "settings-mono", "http://127.0.0.1:7777/callback" }
                }
                div { class: "settings-actions",
                    if spotify_connected {
                        Button {
                            label: "Disconnect".to_string(),
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            on_click: {
                                let sp = sp.clone();
                                move |_| {
                                    let sp = sp.clone();
                                    spawn(async move {
                                        match sp.disconnect().await {
                                            Ok(()) => spotify_status.set(Some("Disconnected.".into())),
                                            Err(e) => spotify_status.set(Some(format!("Disconnect failed: {e}"))),
                                        }
                                    });
                                }
                            },
                        }
                    } else {
                        Button {
                            label: if *is_connecting.read() { "Waiting for browser…".to_string() } else { "Connect".to_string() },
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Sm,
                            disabled: *is_connecting.read() || provider_client_id.is_empty(),
                            on_click: {
                                let sp = sp.clone();
                                move |_| {
                                    let sp = sp.clone();
                                    is_connecting.set(true);
                                    spotify_status.set(Some("Browser tab open — finish sign-in there.".into()));
                                    spawn(async move {
                                        match sp.connect().await {
                                            Ok(()) => spotify_status.set(Some("Connected.".into())),
                                            Err(e) => spotify_status.set(Some(format!("Connect failed: {e}"))),
                                        }
                                        is_connecting.set(false);
                                    });
                                }
                            },
                        }
                    }
                    if provider_client_id.is_empty() {
                        span { class: "settings-status", "Save a Client ID first." }
                    }
                }
                if let Some(msg) = spotify_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }

            SettingsCard {
                title: "SoundCloud".to_string(),
                icon: "fa-brands fa-soundcloud".to_string(),
                status_label: if sc_ready { "Ready".to_string() } else { "Auto-detect".to_string() },
                status_ok: sc_ready,
                p { class: "settings-card-copy",
                    "No login needed. nira extracts and caches the public web-player client_id, then refreshes it after auth failures."
                }
                div { class: "settings-actions",
                    Button {
                        label: if *sc_refreshing.read() { "Refreshing…".to_string() } else { "Refresh client_id".to_string() },
                        icon: Some(if *sc_refreshing.read() { "fa-solid fa-circle-notch fa-spin".to_string() } else { "fa-solid fa-rotate".to_string() }),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: *sc_refreshing.read(),
                        on_click: {
                            let sc = sc.clone();
                            move |_| {
                                let sc = sc.clone();
                                sc_refreshing.set(true);
                                sc_status.set(Some("Refreshing SoundCloud client_id…".into()));
                                spawn(async move {
                                    match sc.refresh_client_id().await {
                                        Ok(_) => sc_status.set(Some("SoundCloud client_id refreshed.".into())),
                                        Err(e) => sc_status.set(Some(format!("Refresh failed: {e}"))),
                                    }
                                    sc_refreshing.set(false);
                                });
                            }
                        },
                    }
                }
                if let Some(msg) = sc_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }

            SettingsCard {
                title: "ListenBrainz".to_string(),
                icon: "fa-solid fa-wave-square".to_string(),
                status_label: if lb_enabled { "Configured".to_string() } else { "Optional".to_string() },
                status_ok: lb_enabled,
                p { class: "settings-card-copy",
                    "Token enables scrobbling. Username enables Home's recent-listens feed. Discovery similarity works without a personal account."
                }
                div { class: "settings-row two-col",
                    label { r#for: "lb-user", "Username" }
                    input {
                        id: "lb-user",
                        r#type: "text",
                        class: "settings-input",
                        placeholder: "your-listenbrainz-username",
                        value: "{lb_user_draft.read()}",
                        oninput: move |e| lb_user_draft.set(e.value()),
                    }
                }
                div { class: "settings-row",
                    label { r#for: "lb-token", "User token" }
                    input {
                        id: "lb-token",
                        r#type: "password",
                        class: "settings-input",
                        placeholder: "lb-token from your profile page",
                        value: "{lb_token_draft.read()}",
                        oninput: move |e| lb_token_draft.set(e.value()),
                    }
                    Button {
                        label: "Save".to_string(),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        on_click: move |_| {
                            let token = lb_token_draft.read().trim().to_string();
                            let user = lb_user_draft.read().trim().to_string();
                            let save_result = {
                                let mut w = config.write();
                                w.listenbrainz_token = if token.is_empty() { None } else { Some(token) };
                                w.listenbrainz_username = if user.is_empty() { None } else { Some(user) };
                                w.save()
                            };
                            match save_result {
                                Ok(()) => lb_status.set(Some("Saved. Scrobbling and Home feed pick this up live.".into())),
                                Err(e) => lb_status.set(Some(format!("Save failed: {e}"))),
                            }
                        },
                    }
                }
                div { class: "settings-meta-grid",
                    StatusPill { label: if token_active { "Scrobbling on".to_string() } else { "Scrobbling off".to_string() }, ok: token_active }
                    StatusPill { label: if username_active { "Home feed on".to_string() } else { "Home feed off".to_string() }, ok: username_active }
                }
                if let Some(msg) = lb_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }
        }
    }
}

#[component]
fn LibrarySettings() -> Element {
    let mut config = use_config();
    let initial_root = config
        .read()
        .library_root
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut root_draft = use_signal(move || initial_root);
    let mut status = use_signal(|| None::<String>);
    let active_root = config
        .read()
        .library_root
        .as_ref()
        .map(|p| p.display().to_string());

    rsx! {
        section { class: "settings-group",
            h2 { "Library" }
            SettingsCard {
                title: "Local music folder".to_string(),
                icon: "fa-solid fa-folder".to_string(),
                status_label: if active_root.is_some() { "Saved".to_string() } else { "Upcoming".to_string() },
                status_ok: active_root.is_some(),
                p { class: "settings-card-copy",
                    "Stored now for the upcoming local scanner, tag index and file playback. Streaming sources are unaffected."
                }
                div { class: "settings-row",
                    label { r#for: "library-root", "Music folder" }
                    input {
                        id: "library-root",
                        r#type: "text",
                        class: "settings-input",
                        placeholder: "/home/you/Music",
                        value: "{root_draft.read()}",
                        oninput: move |e| root_draft.set(e.value()),
                    }
                    Button {
                        label: "Save".to_string(),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        on_click: move |_| {
                            let raw = root_draft.read().trim().to_string();
                            let next = if raw.is_empty() {
                                None
                            } else {
                                let path = PathBuf::from(&raw);
                                if !path.is_dir() {
                                    status.set(Some("That path is not an existing folder.".into()));
                                    return;
                                }
                                Some(path)
                            };
                            let save_result = {
                                let mut w = config.write();
                                w.library_root = next.clone();
                                w.save()
                            };
                            match save_result {
                                Ok(()) => status.set(Some(if next.is_some() {
                                    "Saved. Local scanner/playback will use this folder once implemented.".into()
                                } else {
                                    "Cleared library folder.".into()
                                })),
                                Err(e) => status.set(Some(format!("Save failed: {e}"))),
                            }
                        },
                    }
                }
                if let Some(path) = active_root.as_ref() {
                    p { class: "settings-status ok",
                        i { class: "fa-solid fa-check" }
                        " Saved: "
                        code { class: "settings-mono", "{path}" }
                    }
                }
                if let Some(msg) = status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }
        }
    }
}

#[component]
fn DiscoverySettings() -> Element {
    let mut config = use_config();
    let enrichment = use_enrichment();
    let engine = use_discovery_engine();
    let initial_key = config.read().lastfm_api_key.clone().unwrap_or_default();
    let mut key_draft = use_signal(move || initial_key);
    let mut status = use_signal(|| None::<String>);
    let active_key = enrichment.lastfm_key();
    let saved_key = config
        .read()
        .lastfm_api_key
        .clone()
        .filter(|k| !k.trim().is_empty());
    let lastfm_on = active_key.is_some();
    let sc_source_on = config.read().discovery_soundcloud;
    let lb_source_on = config.read().discovery_listenbrainz;
    let lf_source_on = config.read().discovery_lastfm;

    rsx! {
        section { class: "settings-group settings-stack",
            h2 { "Discovery" }
            p { class: "hint", "Sources behind Similar-to, Radio and Home recommendations." }

            div { class: "settings-meta-grid source-grid",
                StatusPill { label: if sc_source_on { "SoundCloud on".to_string() } else { "SoundCloud off".to_string() }, ok: sc_source_on }
                StatusPill { label: if lb_source_on { "ListenBrainz on".to_string() } else { "ListenBrainz off".to_string() }, ok: lb_source_on }
                StatusPill { label: if lf_source_on && lastfm_on { "Last.fm on".to_string() } else if lf_source_on { "Last.fm needs key".to_string() } else { "Last.fm off".to_string() }, ok: lf_source_on && lastfm_on }
            }

            SettingsCard {
                title: "Recommendation mix".to_string(),
                icon: "fa-solid fa-sliders".to_string(),
                status_label: if lb_source_on { "Blended".to_string() } else { "Aegis-style".to_string() },
                status_ok: sc_source_on,
                p { class: "settings-card-copy",
                    "Radio and Similar-to use these sources live. Default is SoundCloud + Last.fm; ListenBrainz is opt-in because it can drift broader."
                }
                div { class: "source-toggle-grid",
                    SourceToggle {
                        label: "SoundCloud".to_string(),
                        description: "Related tracks, closest to the old Aegis feel.".to_string(),
                        enabled: sc_source_on,
                        recommended: true,
                        on_toggle: {
                            let engine = engine.clone();
                            move |_| match save_discovery_sources(config, engine.clone(), !sc_source_on, lb_source_on, lf_source_on) {
                                Ok(()) => status.set(Some("Discovery sources saved. Radio uses this immediately.".into())),
                                Err(e) => status.set(Some(e)),
                            }
                        },
                    }
                    SourceToggle {
                        label: "Last.fm".to_string(),
                        description: "Similar-track graph from your API key; good fallback signal.".to_string(),
                        enabled: lf_source_on,
                        recommended: true,
                        on_toggle: {
                            let engine = engine.clone();
                            move |_| match save_discovery_sources(config, engine.clone(), sc_source_on, lb_source_on, !lf_source_on) {
                                Ok(()) => status.set(Some("Discovery sources saved. Radio uses this immediately.".into())),
                                Err(e) => status.set(Some(e)),
                            }
                        },
                    }
                    SourceToggle {
                        label: "ListenBrainz".to_string(),
                        description: "Broader co-listening graph. Off by default.".to_string(),
                        enabled: lb_source_on,
                        recommended: false,
                        on_toggle: {
                            let engine = engine.clone();
                            move |_| match save_discovery_sources(config, engine.clone(), sc_source_on, !lb_source_on, lf_source_on) {
                                Ok(()) => status.set(Some("Discovery sources saved. Radio uses this immediately.".into())),
                                Err(e) => status.set(Some(e)),
                            }
                        },
                    }
                }
            }

            SettingsCard {
                title: "Last.fm".to_string(),
                icon: "fa-brands fa-lastfm".to_string(),
                status_label: if lastfm_on { "Enabled".to_string() } else { "Optional".to_string() },
                status_ok: lastfm_on,
                p { class: "settings-card-copy",
                    "Optional app API key for a third recommendation signal. Empty config falls back to "
                    code { "NIRA_LASTFM_API_KEY" }
                    "."
                }
                div { class: "settings-row",
                    label { r#for: "lastfm-key", "API key" }
                    input {
                        id: "lastfm-key",
                        r#type: "password",
                        class: "settings-input",
                        placeholder: "Last.fm API key",
                        value: "{key_draft.read()}",
                        oninput: move |e| key_draft.set(e.value()),
                    }
                    Button {
                        label: "Save".to_string(),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        on_click: {
                            let enrichment = enrichment.clone();
                            move |_| {
                                let key = key_draft.read().trim().to_string();
                                let stored = if key.is_empty() { None } else { Some(key) };
                                let save_result = {
                                    let mut w = config.write();
                                    w.lastfm_api_key = stored.clone();
                                    w.save()
                                };
                                match save_result {
                                    Ok(()) => {
                                        enrichment.set_lastfm_key(stored);
                                        status.set(Some("Saved. Discovery uses the current Last.fm key immediately.".into()));
                                    }
                                    Err(e) => status.set(Some(format!("Save failed: {e}"))),
                                }
                            }
                        },
                    }
                }
                if lastfm_on {
                    p { class: "settings-status ok",
                        i { class: "fa-solid fa-check" }
                        if saved_key.is_some() {
                            " Last.fm source enabled from config."
                        } else {
                            " Last.fm source enabled from environment."
                        }
                    }
                }
                if let Some(msg) = status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }
        }
    }
}

#[component]
fn SourceToggle(
    label: String,
    description: String,
    enabled: bool,
    recommended: bool,
    on_toggle: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            class: if enabled { "source-toggle on" } else { "source-toggle" },
            onclick: move |_| on_toggle.call(()),
            span { class: "source-toggle-icon",
                if enabled {
                    i { class: "fa-solid fa-check" }
                } else {
                    i { class: "fa-solid fa-plus" }
                }
            }
            span { class: "source-toggle-copy",
                strong { "{label}" }
                small { "{description}" }
                if recommended {
                    em { "recommended" }
                }
            }
        }
    }
}

fn save_discovery_sources(
    mut config: Signal<AppConfig>,
    engine: Arc<DiscoveryEngine>,
    soundcloud: bool,
    listenbrainz: bool,
    lastfm: bool,
) -> Result<(), String> {
    if !soundcloud && !listenbrainz && !lastfm {
        return Err("At least one discovery source must stay enabled.".into());
    }
    let save_result = {
        let mut w = config.write();
        w.discovery_soundcloud = soundcloud;
        w.discovery_listenbrainz = listenbrainz;
        w.discovery_lastfm = lastfm;
        w.save()
    };
    save_result.map_err(|e| format!("Save failed: {e}"))?;
    engine.set_source_prefs(DiscoverySourcePrefs {
        soundcloud,
        listenbrainz,
        lastfm,
    });
    Ok(())
}

#[component]
fn DataSettings() -> Element {
    let enrichment = use_enrichment();
    let player = use_player();
    let sc = use_soundcloud();
    let mut status = use_signal(|| None::<String>);
    let config_dir = AppConfig::config_dir();
    let cache_dir = AppConfig::cache_dir();
    let config_label = format_path(config_dir.as_ref());
    let cache_label = format_path(cache_dir.as_ref());

    rsx! {
        section { class: "settings-group settings-stack",
            h2 { "Data" }
            p { class: "hint", "Open local app folders or clear rebuildable cache files." }

            div { class: "settings-data-row",
                div {
                    span { class: "settings-info-label", "Config folder" }
                    code { class: "settings-mono", "{config_label}" }
                }
                Button {
                    label: "Open".to_string(),
                    icon: Some("fa-solid fa-arrow-up-right-from-square".to_string()),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: move |_| status.set(Some(open_dir(config_dir.clone(), "config folder"))),
                }
            }

            div { class: "settings-data-row",
                div {
                    span { class: "settings-info-label", "Cache folder" }
                    code { class: "settings-mono", "{cache_label}" }
                }
                Button {
                    label: "Open".to_string(),
                    icon: Some("fa-solid fa-arrow-up-right-from-square".to_string()),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: move |_| status.set(Some(open_dir(cache_dir.clone(), "cache folder"))),
                }
            }

            div { class: "settings-actions wrap",
                Button {
                    label: "Clear discovery cache".to_string(),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: {
                        let enrichment = enrichment.clone();
                        move |_| {
                            enrichment.clear_cache();
                            status.set(Some("Discovery cache cleared.".into()));
                        }
                    },
                }
                Button {
                    label: "Clear play history".to_string(),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: {
                        let player = player.clone();
                        move |_| {
                            status.set(Some(match player.clear_history() {
                                Ok(()) => "Cleared play history.".into(),
                                Err(e) => format!("Could not clear play history: {e}"),
                            }));
                        }
                    },
                }
                Button {
                    label: "Clear Spotify liked cache".to_string(),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: move |_| status.set(Some(remove_file(AppConfig::spotify_liked_cache_path(), "Spotify liked cache"))),
                }
                Button {
                    label: "Clear SoundCloud cache".to_string(),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: {
                        let sc = sc.clone();
                        move |_| {
                            let sc = sc.clone();
                            status.set(Some("Clearing SoundCloud cache…".into()));
                            spawn(async move {
                                match sc.clear_client_id_cache().await {
                                    Ok(()) => status.set(Some("Cleared SoundCloud client_id cache.".into())),
                                    Err(e) => status.set(Some(format!("Could not clear SoundCloud cache: {e}"))),
                                }
                            });
                        }
                    },
                }
            }

            if let Some(msg) = status.read().as_ref() {
                p { class: "settings-status", "{msg}" }
            }
        }
    }
}

#[component]
fn SettingsCard(
    title: String,
    icon: String,
    status_label: String,
    status_ok: bool,
    children: Element,
) -> Element {
    rsx! {
        article { class: "settings-card",
            header { class: "settings-card-head",
                div { class: "settings-titleline",
                    i { class: "{icon}" }
                    h3 { "{title}" }
                }
                StatusPill { label: status_label, ok: status_ok }
            }
            div { class: "settings-card-body", {children} }
        }
    }
}

#[component]
fn StatusPill(label: String, ok: bool) -> Element {
    let class = if ok {
        "settings-pill ok"
    } else {
        "settings-pill"
    };
    rsx! {
        span { class: "{class}",
            span { class: "settings-dot" }
            "{label}"
        }
    }
}

fn format_path(path: Option<&PathBuf>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "unavailable".into())
}

fn open_dir(path: Option<PathBuf>, label: &str) -> String {
    let Some(path) = path else {
        return format!("Could not resolve {label}.");
    };
    if let Err(e) = std::fs::create_dir_all(&path) {
        return format!("Could not create {label}: {e}");
    }
    match open_path(&path) {
        Ok(()) => format!("Opened {label}."),
        Err(e) => format!("Could not open {label}: {e}"),
    }
}

fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");

    cmd.arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn remove_file(path: Option<PathBuf>, label: &str) -> String {
    let Some(path) = path else {
        return format!("Could not resolve {label} path.");
    };
    match std::fs::remove_file(&path) {
        Ok(()) => format!("Cleared {label}."),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            format!("No {label} file to clear.")
        }
        Err(e) => format!("Could not clear {label}: {e}"),
    }
}
