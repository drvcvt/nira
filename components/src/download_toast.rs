//! One shell-level status toast for background downloads.

use dioxus::prelude::*;
use hooks::use_downloads;

#[component]
pub fn DownloadToast() -> Element {
    let downloads = use_downloads();
    let status = downloads.status.read().clone();
    let busy = *downloads.busy.read();
    let failed = *downloads.failed.read();

    let Some(message) = status else {
        return rsx! {};
    };

    rsx! {
        div {
            class: "download-toast",
            role: "status",
            "aria-live": "polite",
            if busy {
                i { class: "fa-solid fa-circle-notch fa-spin download-toast-glyph" }
            } else if failed {
                i { class: "fa-solid fa-circle-exclamation download-toast-glyph" }
            } else {
                i { class: "fa-solid fa-circle-check download-toast-glyph" }
            }
            span { class: "download-toast-msg", "{message}" }
            if !busy {
                button {
                    class: "download-toast-close",
                    title: "Dismiss",
                    "aria-label": "Dismiss download status",
                    onclick: move |_| downloads.dismiss(),
                    i { class: "fa-solid fa-xmark" }
                }
            }
        }
    }
}
