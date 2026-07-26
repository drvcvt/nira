//! Reactive surface for the user's ListenBrainz listens feed.
//!
//! Fetches `enrichment::EnrichmentClient::lb_user_listens` on mount and
//! whenever the configured username changes. The enrichment client already
//! caches identical (username, limit) requests for 5 minutes, so this hook
//! is safe to re-mount without thrashing the network.

use std::sync::Arc;

use config::AppConfig;
use dioxus::prelude::*;
use enrichment::{EnrichmentClient, Listen};

/// How many lines the Home "Listened lately" section shows.
const FEED_LIMIT: u32 = 10;

#[derive(Clone, PartialEq)]
pub struct UseListenBrainzFeed {
    pub listens: Signal<Vec<Listen>>,
    pub is_loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// `true` when the user hasn't filled in a username yet; the page renders
    /// a "configure-in-settings" CTA instead of an error.
    pub needs_config: Signal<bool>,
}

pub fn use_listenbrainz_feed() -> UseListenBrainzFeed {
    let enrichment = use_context::<Arc<EnrichmentClient>>();
    let config = use_context::<Signal<AppConfig>>();

    let listens = use_signal(Vec::<Listen>::new);
    let is_loading = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let needs_config = use_signal(|| true);
    // Which username the current state belongs to. Outer `None` means "the
    // effect has never applied anything yet", so the first run always fires.
    // The effect below reads the whole `AppConfig` signal, which is also
    // written by `set_volume` — without this guard a volume drag re-spawned
    // one fetch per slider step.
    let mut applied_for = use_signal(|| None::<Option<String>>);

    // Reactive on config changes: when the user types/saves a new username,
    // `use_effect` re-runs and re-fetches.
    use_effect({
        let enrichment = enrichment.clone();
        move || {
            let username = config
                .read()
                .listenbrainz_username
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            // Only the username field matters here — any other config write
            // (volume, theme, …) must not re-fetch.
            if applied_for.peek().as_ref() == Some(&username) {
                return;
            }
            applied_for.set(Some(username.clone()));

            let mut listens_sig = listens;
            let mut loading_sig = is_loading;
            let mut error_sig = error;
            let mut needs_config_sig = needs_config;

            let Some(username) = username else {
                needs_config_sig.set(true);
                listens_sig.set(Vec::new());
                error_sig.set(None);
                return;
            };
            needs_config_sig.set(false);

            let enrichment = enrichment.clone();
            spawn(async move {
                loading_sig.set(true);
                error_sig.set(None);
                match enrichment.lb_user_listens(&username, FEED_LIMIT).await {
                    Ok(rows) => listens_sig.set(rows),
                    Err(e) => error_sig.set(Some(e.to_string())),
                }
                loading_sig.set(false);
            });
        }
    });

    UseListenBrainzFeed {
        listens,
        is_loading,
        error,
        needs_config,
    }
}
