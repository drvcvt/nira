//! Reactive surface for the on-disk music folder.
//!
//! A context singleton (installed once from the root) so the scan runs a
//! single time on boot rather than on every Library-tab mount. The actual
//! filesystem walk + tag parse runs on a blocking thread via
//! `spawn_blocking`; for a few thousand files that's a second or two we must
//! keep off the Dioxus executor or the UI stutters. `rescan()` re-reads the
//! configured root and walks again — wired to the Settings "Save" button and
//! the Library tab's refresh control.

use config::AppConfig;
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use provider_api::Track;

#[derive(Clone, Copy)]
pub struct UseLocalLibrary {
    pub tracks: Signal<Vec<Track>>,
    pub is_scanning: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// Total on-disk size of the scanned library, in bytes. Lets the UI show
    /// how much space local music is using (headroom before the external SSD).
    pub total_bytes: Signal<u64>,
    config: Signal<AppConfig>,
}

impl UseLocalLibrary {
    pub fn list(&self) -> Vec<Track> {
        self.tracks.read().clone()
    }

    pub fn count(&self) -> usize {
        self.tracks.read().len()
    }

    /// Re-read the configured `library_root` and walk it again. No-op-ish
    /// when no folder is set: clears the list so the UI drops back to its
    /// empty state.
    pub fn rescan(&self) {
        scan_into(*self);
    }
}

fn scan_into(state: UseLocalLibrary) {
    let mut tracks = state.tracks;
    let mut scanning = state.is_scanning;
    let mut error = state.error;
    let mut total_bytes = state.total_bytes;

    let Some(root) = state.config.peek().library_root.clone() else {
        tracks.set(Vec::new());
        total_bytes.set(0);
        error.set(None);
        scanning.set(false);
        return;
    };

    scanning.set(true);
    error.set(None);
    // spawn_forever: rescan() gets called from page scopes (Settings save,
    // Library refresh) — a scope-owned spawn dies when that page unmounts,
    // stranding `is_scanning` at true ("Scanning…" forever).
    spawn_forever(async move {
        // Disk walk + lofty tag parse is blocking work — keep it off the
        // Dioxus executor so the UI stays responsive during the scan. Total
        // size is summed in the same blocking task (one stat per file).
        let covers_dir = AppConfig::covers_cache_dir();
        let result = tokio::task::spawn_blocking(move || {
            let list = provider_local::scan(&root, covers_dir.as_deref());
            let bytes = provider_local::total_size_bytes(&list);
            (list, bytes)
        })
        .await;
        match result {
            Ok((list, bytes)) => {
                tracks.set(list);
                total_bytes.set(bytes);
            }
            Err(e) => error.set(Some(format!("Library scan failed: {e}"))),
        }
        scanning.set(false);
    });
}

/// Install the singleton and kick the boot-time scan. Called once from
/// `AppContext::install`, after the config signal exists.
pub fn install_local_library(config: Signal<AppConfig>) {
    // Root-owned signals (not use_signal): scan_into writes them from a
    // spawn_forever task, which runs on ScopeId::ROOT — an ancestor of the
    // App scope. Component-owned signals written from there trip Dioxus's
    // "used in a non-descendant scope" warning and risk a dropped-value read.
    let state = use_hook(|| UseLocalLibrary {
        tracks: Signal::new_in_scope(Vec::new(), ScopeId::ROOT),
        is_scanning: Signal::new_in_scope(false, ScopeId::ROOT),
        error: Signal::new_in_scope(None, ScopeId::ROOT),
        total_bytes: Signal::new_in_scope(0, ScopeId::ROOT),
        config,
    });
    use_context_provider(move || state);
    use_hook(move || scan_into(state));
}

pub fn use_local_library() -> UseLocalLibrary {
    use_context::<UseLocalLibrary>()
}
