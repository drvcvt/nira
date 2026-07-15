//! Provider credentials & service status: Spotify, SoundCloud, ListenBrainz.

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{FLAC_QUALITIES, use_config, use_hires-provider, use_soundcloud, use_spotify};

use super::SettingsCard;

#[component]
pub(super) fn ConnectionsSettings() -> Element {
    let mut config = use_config();
    let sp = use_spotify();
    let sc = use_soundcloud();
    let qz = use_hires-provider();

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

    // the hi-res provider: token-based auth (email/password login was disabled by the hi-res provider).
    let qz_initial_token = config.read().hires-provider_token.clone().unwrap_or_default();
    let mut qz_token = use_signal(move || qz_initial_token);
    let mut qz_format = use_signal(|| config.read().hires-provider_format_id.unwrap_or(27));
    let mut qz_status = use_signal(|| None::<String>);
    let mut qz_busy = use_signal(|| false);
    let qz_connected = qz.is_connected();

    rsx! {
        section { class: "settings-group settings-stack",
            h2 { "Connections" }
            p { class: "hint", "Provider credentials and service status." }

            SettingsCard {
                title: "the hi-res provider".to_string(),
                icon: "fa-solid fa-compact-disc".to_string(),
                p { class: "settings-card-copy",
                    "Download hi-res FLAC from your the hi-res provider Studio subscription into your library. the hi-res provider disabled email/password login for third-party apps, so nira uses your session token instead."
                }
                div { class: "settings-warn",
                    strong { "How to get your token: " }
                    "Open "
                    code { "the provider web player" }
                    " (logged in) → press F12 → Network tab → reload → click any "
                    code { "api.json" }
                    " request → under Request Headers copy the value of "
                    code { "auth token" }
                    ". Paste it below."
                }
                div { class: "settings-row",
                    label { r#for: "qz-token", "Auth token" }
                    input {
                        id: "qz-token",
                        r#type: "password",
                        class: "settings-input",
                        placeholder: "auth token from the provider web player",
                        value: "{qz_token.read()}",
                        disabled: *qz_busy.read(),
                        oninput: move |e| qz_token.set(e.value()),
                    }
                }
                div { class: "settings-row compact",
                    label { r#for: "qz-quality", "Quality" }
                    select {
                        id: "qz-quality",
                        class: "settings-input",
                        disabled: *qz_busy.read(),
                        onchange: {
                            let qz = qz.clone();
                            move |e: Event<FormData>| {
                                let fid: u32 = e.value().parse().unwrap_or(27);
                                qz_format.set(fid);
                                let mut w = config.write();
                                w.hires-provider_format_id = Some(fid);
                                let _ = w.save();
                                let qz = qz.clone();
                                spawn(async move { qz.set_format_id(fid).await; });
                            }
                        },
                        for (fid, label) in FLAC_QUALITIES.iter() {
                            option {
                                value: "{fid}",
                                selected: *qz_format.read() == *fid,
                                "{label}"
                            }
                        }
                    }
                }
                div { class: "settings-actions",
                    if qz_connected {
                        Button {
                            label: "Disconnect".to_string(),
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            on_click: {
                                let qz = qz.clone();
                                move |_| {
                                    let qz = qz.clone();
                                    {
                                        let mut w = config.write();
                                        w.hires-provider_token = None;
                                        let _ = w.save();
                                    }
                                    qz_token.set(String::new());
                                    spawn(async move {
                                        qz.logout().await;
                                        qz_status.set(Some("Disconnected.".into()));
                                    });
                                }
                            },
                        }
                    } else {
                        Button {
                            label: if *qz_busy.read() { "Verifying…".to_string() } else { "Connect".to_string() },
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Sm,
                            disabled: *qz_busy.read(),
                            on_click: {
                                let qz = qz.clone();
                                move |_| {
                                    let token = qz_token.read().trim().to_string();
                                    if token.is_empty() {
                                        qz_status.set(Some("Paste your the hi-res provider auth token first.".into()));
                                        return;
                                    }
                                    qz_busy.set(true);
                                    qz_status.set(Some("Verifying token with the hi-res provider…".into()));
                                    let qz = qz.clone();
                                    spawn(async move {
                                        match qz.set_token(&token).await {
                                            Ok(()) => {
                                                let mut w = config.write();
                                                w.hires-provider_token = Some(token.clone());
                                                let _ = w.save();
                                                drop(w);
                                                qz_status.set(Some("Connected. Download FLACs from search.".into()));
                                            }
                                            Err(e) => qz_status.set(Some(format!("Token rejected: {e}. Make sure you copied the full auth token while logged in."))),
                                        }
                                        qz_busy.set(false);
                                    });
                                }
                            },
                        }
                    }
                }
                if let Some(msg) = qz_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }

            SettingsCard {
                title: "Spotify".to_string(),
                icon: "fa-brands fa-spotify".to_string(),
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
                if let Some(msg) = lb_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }
        }
    }
}
