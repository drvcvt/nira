//! Multi-provider search. Owns query/results/search-side error state only —
//! playback dispatch lives in `queue.rs`. Pages call
//! `queue.play_list(results, idx)` on click; this hook just produces the
//! list.

use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;
use provider_api::{Provider, Query, SearchResults, Track};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

#[derive(Clone)]
pub struct UseSearch {
    pub query: Signal<String>,
    pub results: Signal<Vec<Track>>,
    pub is_searching: Signal<bool>,
    pub error: Signal<Option<String>>,
}

pub fn use_search() -> UseSearch {
    let sc = use_context::<Arc<SoundCloudProvider>>();
    let sp = use_context::<Arc<SpotifyProvider>>();

    let query = use_signal(String::new);
    let results = use_signal(Vec::<Track>::new);
    let is_searching = use_signal(|| false);
    let error = use_signal(|| None::<String>);

    use_effect({
        let sc = sc.clone();
        let sp = sp.clone();
        move || {
            let snapshot = query.read().clone();
            let sc = sc.clone();
            let sp = sp.clone();
            let q_sig = query;
            let mut results_sig = results;
            let mut is_searching_sig = is_searching;
            let mut error_sig = error;

            spawn(async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if *q_sig.peek() != snapshot {
                    return;
                }
                let trimmed = snapshot.trim();
                if trimmed.is_empty() {
                    results_sig.set(Vec::new());
                    is_searching_sig.set(false);
                    error_sig.set(None);
                    return;
                }
                is_searching_sig.set(true);
                error_sig.set(None);
                let q = Query {
                    text: trimmed.to_string(),
                    limit: Some(15),
                };

                let sp_connected = sp.is_connected();
                let q_sc = q.clone();
                let q_sp = q.clone();
                let (sc_res, sp_res) = tokio::join!(sc.search(&q_sc), async move {
                    if sp_connected {
                        sp.search(&q_sp).await
                    } else {
                        Ok(SearchResults::default())
                    }
                },);

                if *q_sig.peek() != snapshot {
                    is_searching_sig.set(false);
                    return;
                }

                match (sc_res, sp_res) {
                    (Ok(sc_r), Ok(sp_r)) => {
                        results_sig.set(interleave(sp_r.tracks, sc_r.tracks));
                    }
                    (Ok(sc_r), Err(e)) => {
                        tracing::warn!(error=%e, "Spotify search failed; falling back to SC only");
                        results_sig.set(sc_r.tracks);
                    }
                    (Err(e), Ok(sp_r)) => {
                        tracing::warn!(error=%e, "SoundCloud search failed; falling back to Spotify only");
                        results_sig.set(sp_r.tracks);
                    }
                    (Err(sc_e), Err(sp_e)) => {
                        error_sig.set(Some(format!(
                            "both providers failed — sc: {sc_e}; spotify: {sp_e}"
                        )));
                    }
                }
                is_searching_sig.set(false);
            });
        }
    });

    UseSearch {
        query,
        results,
        is_searching,
        error,
    }
}

fn interleave(a: Vec<Track>, b: Vec<Track>) -> Vec<Track> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let mut a = a.into_iter();
    let mut b = b.into_iter();
    loop {
        let (x, y) = (a.next(), b.next());
        if x.is_none() && y.is_none() {
            break;
        }
        if let Some(t) = x {
            out.push(t);
        }
        if let Some(t) = y {
            out.push(t);
        }
    }
    out
}
