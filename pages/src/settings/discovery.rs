//! Discovery sources (SoundCloud / Last.fm / ListenBrainz) and the Last.fm
//! API key.

use std::sync::Arc;

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{
    AppConfig, DiscoveryEngine, DiscoverySourcePrefs, use_config, use_discovery_engine,
    use_enrichment,
};

use super::SettingsCard;

#[component]
pub(super) fn DiscoverySettings() -> Element {
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

            SettingsCard {
                title: "Recommendation mix".to_string(),
                icon: "fa-solid fa-sliders".to_string(),
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
