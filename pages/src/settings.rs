use std::path::PathBuf;

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{use_config, use_enrichment, use_spotify};

#[component]
pub fn Settings() -> Element {
    let mut config = use_config();
    let sp = use_spotify();

    let mut client_id_draft = use_signal({
        let cfg = config.read();
        move || cfg.spotify_client_id.clone().unwrap_or_default()
    });
    let mut save_status = use_signal(|| None::<String>);
    let mut connect_status = use_signal(|| None::<String>);
    let mut is_connecting = use_signal(|| false);

    let provider_client_id = sp.client_id();
    let is_connected = sp.is_connected();

    rsx! {
        section { class: "page",
            h1 { "Settings" }
            p { class: "hint", "Audio device, library paths, providers." }

            // ── Local library ────────────────────────────────────────────
            LibrarySettings {}

            // ── Spotify ──────────────────────────────────────────────────
            section { class: "settings-group",
                h2 { "Spotify" }
                p { class: "hint",
                    "nira speaks to Spotify via OAuth (PKCE). You need your own "
                    "Spotify Developer app — there's no shared key. Register at "
                    "developer.spotify.com/dashboard, add the redirect URI "
                    "below verbatim, copy the Client ID here."
                }

                div { class: "settings-row",
                    label { "Redirect URI (paste into your Spotify app)" }
                    code { class: "settings-mono", "http://127.0.0.1:7777/callback" }
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
                                        save_status.set(Some("Saved. Spotify uses this Client ID immediately.".into()));
                                        let sp = sp.clone();
                                        spawn(async move {
                                            match sp.set_client_id(new_id).await {
                                                Ok(true) => connect_status.set(Some("Client ID changed; old Spotify session was cleared. Connect again if needed.".into())),
                                                Ok(false) => {}
                                                Err(e) => connect_status.set(Some(format!("Provider update failed: {e}"))),
                                            }
                                        });
                                    }
                                    Err(e) => save_status.set(Some(format!("Save failed: {e}"))),
                                }
                            }
                        },
                    }
                }
                if let Some(msg) = save_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }

                div { class: "settings-row",
                    if is_connected {
                        Button {
                            label: "Disconnect".to_string(),
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            on_click: {
                                let sp = sp.clone();
                                move |_| {
                                    let sp = sp.clone();
                                    spawn(async move {
                                        let _ = sp.disconnect().await;
                                    });
                                    connect_status.set(Some("Disconnected.".into()));
                                }
                            },
                        }
                        span { class: "settings-status ok", "Connected." }
                    } else {
                        Button {
                            label: if *is_connecting.read() { "Waiting for browser…".to_string() } else { "Connect to Spotify".to_string() },
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Sm,
                            disabled: *is_connecting.read() || provider_client_id.is_empty(),
                            on_click: {
                                let sp = sp.clone();
                                move |_| {
                                    let sp = sp.clone();
                                    is_connecting.set(true);
                                    connect_status.set(Some("Browser tab open — finish sign-in there.".into()));
                                    spawn(async move {
                                        match sp.connect().await {
                                            Ok(()) => connect_status.set(Some("Connected.".into())),
                                            Err(e) => connect_status.set(Some(format!("Connect failed: {e}"))),
                                        }
                                        is_connecting.set(false);
                                    });
                                }
                            },
                        }
                        if provider_client_id.is_empty() {
                            span { class: "settings-status", "Save a Client ID first." }
                        }
                    }
                }
                if let Some(msg) = connect_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }

            // ── SoundCloud ───────────────────────────────────────────────
            section { class: "settings-group",
                h2 { "SoundCloud" }
                p { class: "hint",
                    "No setup needed — nira extracts a client_id from the public "
                    "web player at startup and refreshes it automatically when SC "
                    "rotates it. If searches stop working, restart usually fixes it."
                }
            }

            // ── ListenBrainz ─────────────────────────────────────────────
            ListenBrainzSettings {}

            // ── Last.fm ──────────────────────────────────────────────────
            LastFmSettings {}
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
            h2 { "Local library" }
            p { class: "hint",
                "Path is stored now; scanner, tag index and local playback are still the next feature step. "
                "This makes the later local-library work configurable without pretending it already plays files."
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

            if let Some(msg) = status.read().as_ref() {
                p { class: "settings-status", "{msg}" }
            }
            if let Some(path) = active_root.as_ref() {
                p { class: "settings-status ok",
                    i { class: "fa-solid fa-check" }
                    " Saved: "
                    code { class: "settings-mono", "{path}" }
                }
            }
        }
    }
}

#[component]
fn ListenBrainzSettings() -> Element {
    let mut config = use_config();
    let initial_token = config.read().listenbrainz_token.clone().unwrap_or_default();
    let initial_user = config
        .read()
        .listenbrainz_username
        .clone()
        .unwrap_or_default();
    let mut token_draft = use_signal(move || initial_token);
    let mut user_draft = use_signal(move || initial_user);
    let mut status = use_signal(|| None::<String>);
    let token_active = config
        .read()
        .listenbrainz_token
        .clone()
        .filter(|t| !t.trim().is_empty());
    let username_active = config
        .read()
        .listenbrainz_username
        .clone()
        .filter(|u| !u.trim().is_empty());

    rsx! {
        section { class: "settings-group",
            h2 { "ListenBrainz" }
            p { class: "hint",
                "Optional. Paste your user token from "
                a { href: "https://listenbrainz.org/profile/", style: "color: var(--accent-soft)",
                    "listenbrainz.org/profile/" }
                " to scrobble plays. The username drives the read-back side — "
                "Home's \"Listened lately\" row pulls it from "
                code { "/user/<name>/listens" }
                ". Token alone enables scrobbling; both fields enable the Home feed."
            }

            div { class: "settings-row",
                label { r#for: "lb-user", "Username" }
                input {
                    id: "lb-user",
                    r#type: "text",
                    class: "settings-input",
                    placeholder: "your-listenbrainz-username",
                    value: "{user_draft.read()}",
                    oninput: move |e| user_draft.set(e.value()),
                }
            }

            div { class: "settings-row",
                label { r#for: "lb-token", "User token" }
                input {
                    id: "lb-token",
                    r#type: "password",
                    class: "settings-input",
                    placeholder: "lb-token from your profile page",
                    value: "{token_draft.read()}",
                    oninput: move |e| token_draft.set(e.value()),
                }
                Button {
                    label: "Save".to_string(),
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    on_click: move |_| {
                        let token = token_draft.read().trim().to_string();
                        let user = user_draft.read().trim().to_string();
                        let mut w = config.write();
                        w.listenbrainz_token = if token.is_empty() { None } else { Some(token) };
                        w.listenbrainz_username = if user.is_empty() { None } else { Some(user) };
                        match w.save() {
                            Ok(()) => status.set(Some(
                                "Saved. Scrobbling uses the new token immediately; Home picks up the username on next refresh.".into(),
                            )),
                            Err(e) => status.set(Some(format!("Save failed: {e}"))),
                        }
                    },
                }
            }

            if let Some(msg) = status.read().as_ref() {
                p { class: "settings-status", "{msg}" }
            }

            if token_active.is_some() {
                p { class: "settings-status ok",
                    i { class: "fa-solid fa-check" }
                    " Scrobbling enabled."
                }
            }
            if username_active.is_some() {
                p { class: "settings-status ok",
                    i { class: "fa-solid fa-check" }
                    " Home listen feed enabled."
                }
            }
        }
    }
}

#[component]
fn LastFmSettings() -> Element {
    let mut config = use_config();
    let enrichment = use_enrichment();
    let initial_key = config.read().lastfm_api_key.clone().unwrap_or_default();
    let mut key_draft = use_signal(move || initial_key);
    let mut status = use_signal(|| None::<String>);
    let active_key = enrichment.lastfm_key();
    let saved_key = config
        .read()
        .lastfm_api_key
        .clone()
        .filter(|k| !k.trim().is_empty());

    rsx! {
        section { class: "settings-group",
            h2 { "Last.fm" }
            p { class: "hint",
                "Optional app API key for the third Discovery source. Empty config falls back to "
                code { "NIRA_LASTFM_API_KEY" }
                "; saving here updates Discovery immediately."
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

            if let Some(msg) = status.read().as_ref() {
                p { class: "settings-status", "{msg}" }
            }
            if active_key.is_some() {
                p { class: "settings-status ok",
                    i { class: "fa-solid fa-check" }
                    if saved_key.is_some() {
                        " Last.fm Discovery source enabled from config."
                    } else {
                        " Last.fm Discovery source enabled from environment."
                    }
                }
            }
        }
    }
}
