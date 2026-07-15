//! Local data: open config/cache folders, clear rebuildable caches.

use std::path::{Path, PathBuf};
use std::process::Command;

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{AppConfig, use_enrichment, use_player, use_soundcloud};

#[component]
pub(super) fn DataSettings() -> Element {
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
