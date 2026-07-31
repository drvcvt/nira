//! Provider credentials & service status: Spotify, SoundCloud, ListenBrainz,
//! plus the listen-together session.

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    use_config, use_soundcloud, use_spotify, use_together, validate_soundcloud_url,
};
use together::Role;

use super::{SettingsCard, StatusPill};

#[component]
pub(super) fn ConnectionsSettings() -> Element {
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
    // Active state (what config actually holds), not the drafts — the pills
    // flip when Save lands, not while typing.
    let lb_token_active = config.read().listenbrainz_token.is_some();
    let lb_user_active = config.read().listenbrainz_username.is_some();

    let mut sc_status = use_signal(|| None::<String>);
    let mut sc_refreshing = use_signal(|| false);
    let mut discord_status = use_signal(|| None::<String>);
    let discord_presence_on = config.read().discord_presence;
    let mut sc_profile_draft = use_signal({
        let cfg = config.read();
        move || cfg.soundcloud_profile_url.clone().unwrap_or_default()
    });

    let provider_client_id = sp.client_id();
    let spotify_connected = sp.is_connected();


    rsx! {
        section { class: "settings-group settings-stack",
            h2 { "Connections" }
            p { class: "hint", "Provider credentials and service status." }

            SettingsCard {
                title: "Discord activity".to_string(),
                icon: "fa-brands fa-discord".to_string(),
                p { class: "settings-card-copy",
                    "Share the current song, artist, album and available cover on your Discord profile. The playback provider is never shown."
                }
                div { class: "source-toggle-grid",
                    button {
                        class: if discord_presence_on { "source-toggle on" } else { "source-toggle" },
                        "aria-pressed": if discord_presence_on { "true" } else { "false" },
                        onclick: move |_| {
                            let next = !discord_presence_on;
                            let result = {
                                let mut w = config.write();
                                w.discord_presence = next;
                                w.save()
                            };
                            discord_status.set(Some(match result {
                                Ok(()) if next => "Discord activity enabled.".into(),
                                Ok(()) => "Discord activity disabled and cleared.".into(),
                                Err(e) => format!("Changed for this session, but saving failed: {e}"),
                            }));
                        },
                        span { class: "source-toggle-icon",
                            if discord_presence_on {
                                i { class: "fa-solid fa-check" }
                            } else {
                                i { class: "fa-solid fa-plus" }
                            }
                        }
                        span { class: "source-toggle-copy",
                            strong { if discord_presence_on { "Sharing on" } else { "Sharing off" } }
                            small { "Click to change it immediately." }
                        }
                    }
                }
                if let Some(msg) = discord_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }

            ListenTogetherCard {}

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
                div { class: "settings-row",
                    label { r#for: "sc-profile-url", "Import profile" }
                    input {
                        id: "sc-profile-url",
                        r#type: "url",
                        class: "settings-input",
                        placeholder: "https://soundcloud.com/your-profile",
                        value: "{sc_profile_draft.read()}",
                        oninput: move |e| sc_profile_draft.set(e.value()),
                    }
                    Button {
                        label: "Save".to_string(),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        on_click: move |_| {
                            let profile = sc_profile_draft.read().trim().to_string();
                            if !profile.is_empty() {
                                if let Err(e) = validate_soundcloud_url(&profile) {
                                    sc_status.set(Some(e.to_string()));
                                    return;
                                }
                            }
                            let save_result = {
                                let mut w = config.write();
                                w.soundcloud_profile_url =
                                    if profile.is_empty() { None } else { Some(profile) };
                                w.save()
                            };
                            match save_result {
                                Ok(()) => sc_status.set(Some("SoundCloud import profile saved.".into())),
                                Err(e) => sc_status.set(Some(format!("Save failed: {e}"))),
                            }
                        },
                    }
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
                div { class: "settings-meta-grid",
                    StatusPill {
                        label: if lb_token_active { "Scrobbling on".to_string() } else { "Scrobbling off".to_string() },
                        ok: lb_token_active,
                    }
                    StatusPill {
                        label: if lb_user_active { "Home feed on".to_string() } else { "Home feed off".to_string() },
                        ok: lb_user_active,
                    }
                }
                if let Some(msg) = lb_status.read().as_ref() {
                    p { class: "settings-status", "{msg}" }
                }
            }
        }
    }
}

/// Listen together: host a session or join one from a share code.
///
/// The code is the host's iroh address — an ed25519 public key plus relay and
/// direct-address hints. It is not a secret in the "keep it safe" sense, but
/// anyone holding it can attempt to connect, so treat it like a party invite
/// rather than a public link.
#[component]
fn ListenTogetherCard() -> Element {
    let together = use_together();
    let config = use_config();
    let snap = together.snapshot.read().clone();
    let mut code_draft = use_signal(String::new);

    let display_name = config
        .read()
        .listenbrainz_username
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "a friend".to_string());

    let ticket = snap.ticket.clone();
    let status = snap.status.clone();
    let peers = snap.peers.join(", ");

    rsx! {
        SettingsCard {
            title: "Listen together".to_string(),
            icon: "fa-solid fa-user-group".to_string(),
            p { class: "settings-card-copy",
                "Play the same thing at the same time with a friend, peer to peer. "
                "The connection is end-to-end encrypted and goes directly between "
                "your machines where the network allows it. Step one syncs tracks "
                "you both already have in your local library."
            }

            if let Some(code) = ticket {
                div { class: "settings-row",
                    label { r#for: "lt-code", "Your session code" }
                    input {
                        id: "lt-code",
                        r#type: "text",
                        readonly: true,
                        value: "{code}",
                        onclick: move |_| {
                            dioxus::document::eval(
                                "document.getElementById('lt-code').select();"
                            );
                        },
                    }
                }
                p { class: "hint", "Send this to whoever should listen along." }
            } else if snap.role == Role::Off {
                div { class: "settings-row",
                    label { r#for: "lt-join", "Session code" }
                    input {
                        id: "lt-join",
                        r#type: "text",
                        placeholder: "paste a code from a friend",
                        value: "{code_draft}",
                        oninput: move |e| code_draft.set(e.value()),
                    }
                }
            }

            if !peers.is_empty() {
                p { class: "settings-status", "Connected: {peers}" }
            }
            if let Some(missing) = together.unmatched.read().clone() {
                p { class: "settings-status",
                    "Sitting this one out — your friend is playing a file from their "
                    "own library that you don't have: {missing}"
                }
            }
            if !status.is_empty() {
                p { class: "settings-status",
                    "{status}"
                    if let Some(rtt) = snap.rtt_ms {
                        " · {rtt} ms round trip"
                    }
                }
            }

            div { class: "settings-actions",
                if snap.role == Role::Off {
                    Button {
                        label: "Host a session".to_string(),
                        icon: Some("fa-solid fa-tower-broadcast".to_string()),
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Sm,
                        on_click: {
                            let name = display_name.clone();
                            move |_| together.host(name.clone())
                        },
                    }
                    Button {
                        label: "Join".to_string(),
                        icon: Some("fa-solid fa-right-to-bracket".to_string()),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: code_draft.read().trim().is_empty(),
                        on_click: {
                            let name = display_name.clone();
                            move |_| together.join(code_draft.read().trim().to_string(), name.clone())
                        },
                    }
                } else {
                    Button {
                        label: "Leave".to_string(),
                        icon: Some("fa-solid fa-right-from-bracket".to_string()),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        on_click: move |_| together.leave(),
                    }
                }
            }
        }
    }
}
