//! Local music folder — the scan root for provider-local.

use std::path::PathBuf;

use components::{Button, ButtonSize, ButtonVariant};
use dioxus::prelude::*;
use hooks::{use_config, use_local_library};

use super::SettingsCard;

#[component]
pub(super) fn LibrarySettings() -> Element {
    let mut config = use_config();
    let local = use_local_library();
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
                p { class: "settings-card-copy",
                    "Scanned for FLAC, MP3, M4A, OGG and WAV. Tracks show up under Library → Local. Saving rescans immediately; streaming sources are unaffected."
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
                                Ok(()) => {
                                    local.rescan();
                                    status.set(Some(if next.is_some() {
                                        "Saved. Scanning this folder now — check Library → Local.".into()
                                    } else {
                                        "Cleared library folder.".into()
                                    }));
                                }
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
