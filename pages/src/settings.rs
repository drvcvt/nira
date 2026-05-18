use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{use_config, use_spotify};

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

    let saved_client_id = config.read().spotify_client_id.clone();
    let provider_client_id = sp.client_id().to_string();
    let restart_needed = saved_client_id.as_deref().unwrap_or("") != provider_client_id;
    let is_connected = sp.is_connected();

    rsx! {
        section { class: "page",
            h1 { "Settings" }
            p { class: "hint", "Audio device, library paths, providers." }

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
                        oninput: move |e| client_id_draft.set(e.value()),
                    }
                    Button {
                        label: "Save".to_string(),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        on_click: move |_| {
                            let new_id = client_id_draft.read().trim().to_string();
                            let mut cfg_write = config.write();
                            cfg_write.spotify_client_id = if new_id.is_empty() { None } else { Some(new_id) };
                            match cfg_write.save() {
                                Ok(()) => save_status.set(Some("Saved. Restart nira so the Spotify provider picks up the new Client ID.".into())),
                                Err(e) => save_status.set(Some(format!("Save failed: {e}"))),
                            }
                        },
                    }
                }
                if let Some(msg) = save_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }

                if restart_needed && !provider_client_id.is_empty() {
                    p { class: "settings-warn",
                        "Provider is still using Client ID "
                        code { "{provider_client_id}" }
                        " — restart nira to pick up the saved value."
                    }
                } else if restart_needed && provider_client_id.is_empty() {
                    p { class: "settings-warn", "No Client ID active yet — restart nira after saving." }
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
                            span { class: "settings-status", "Save a Client ID and restart first." }
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
