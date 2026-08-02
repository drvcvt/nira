//! Multi-provider search. Owns query/results/search-side error state only —
//! playback dispatch lives in `queue.rs`. Pages call
//! `queue.play_list(results, idx)` on click; this hook just produces the
//! list.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;
use provider_api::{Artist, PlaylistBrief, Provider, Query, SearchResults, Track};
use provider_soundcloud::SoundCloudProvider;
use provider_spotify::SpotifyProvider;

use crate::matching::match_key;

#[derive(Clone)]
pub struct UseSearch {
    pub query: Signal<String>,
    pub results: Signal<Vec<Track>>,
    /// Artist hits across providers, deduped by normalized name. Spotify
    /// entries win the dedupe — their profiles carry albums + related,
    /// which a SoundCloud user page can't offer.
    pub artists: Signal<Vec<Artist>>,
    pub playlists: Signal<Vec<PlaylistBrief>>,
    pub is_searching: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// The query string the current `results` belong to. Lets Enter-to-play
    /// callers refuse to act on results from a previous query (debounce +
    /// fetch lag means `results` trails `query` by up to a second).
    pub results_for: Signal<String>,
}

pub(crate) fn install_search() {
    let sc = use_context::<Arc<SoundCloudProvider>>();
    let sp = use_context::<Arc<SpotifyProvider>>();

    let query = use_signal(String::new);
    let results = use_signal(Vec::<Track>::new);
    let artists = use_signal(Vec::<Artist>::new);
    let playlists = use_signal(Vec::<PlaylistBrief>::new);
    let is_searching = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let results_for = use_signal(String::new);

    use_effect({
        let sc = sc.clone();
        let sp = sp.clone();
        move || {
            let snapshot = query.read().clone();
            let sc = sc.clone();
            let sp = sp.clone();
                let q_sig = query;
            let mut results_sig = results;
            let mut artists_sig = artists;
            let mut playlists_sig = playlists;
            let mut is_searching_sig = is_searching;
            let mut error_sig = error;
            let mut results_for_sig = results_for;

            spawn(async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if *q_sig.peek() != snapshot {
                    return;
                }
                let trimmed = snapshot.trim();
                if trimmed.is_empty() {
                    results_sig.set(Vec::new());
                    artists_sig.set(Vec::new());
                    playlists_sig.set(Vec::new());
                    results_for_sig.set(snapshot.clone());
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
                // Per-provider timing so a slow search points at its culprit.
                let t0 = std::time::Instant::now();
                let (sc_res, sp_res) = tokio::join!(
                    async {
                        let t = std::time::Instant::now();
                        let r = sc.search(&q_sc).await;
                        (r, t.elapsed().as_millis() as u64)
                    },
                    async move {
                        let t = std::time::Instant::now();
                        let r = if sp_connected {
                            sp.search(&q_sp).await
                        } else {
                            Ok(SearchResults::default())
                        };
                        (r, t.elapsed().as_millis() as u64)
                    },
                );
                let (sc_res, sc_ms) = sc_res;
                let (sp_res, sp_ms) = sp_res;
                tracing::info!(
                    total_ms = t0.elapsed().as_millis() as u64,
                    sc_ms,
                    sp_ms,
                    "search completed"
                );

                if *q_sig.peek() != snapshot {
                    // Superseded — the newer search owns the spinner now, so
                    // don't clear it out from under that still-running task.
                    return;
                }

                // Streaming providers interleaved. Any provider that errors
                // just drops out — one failure never blanks the results.
                let (sc_tracks, sc_artists, sc_playlists, sc_err) = match sc_res {
                    Ok(r) => (r.tracks, r.artists, r.playlists, None),
                    Err(e) => (Vec::new(), Vec::new(), Vec::new(), Some(e)),
                };
                let (sp_tracks, sp_artists, sp_playlists, sp_err) = match sp_res {
                    Ok(r) => (r.tracks, r.artists, r.playlists, None),
                    Err(e) => (Vec::new(), Vec::new(), Vec::new(), Some(e)),
                };

                artists_sig.set(merge_artist_hits(sp_artists, sc_artists));

                match (sc_err, sp_err) {
                    (None, None) => {
                        results_sig.set(interleave(sp_tracks, sc_tracks));
                        playlists_sig.set(merge_playlist_hits(sp_playlists, sc_playlists));
                    }
                    (None, Some(e)) => {
                        tracing::warn!(error=%e, "Spotify search failed; falling back to SC only");
                        results_sig.set(sc_tracks);
                        playlists_sig.set(merge_playlist_hits(Vec::new(), sc_playlists));
                    }
                    (Some(e), None) => {
                        tracing::warn!(error=%e, "SoundCloud search failed; falling back to Spotify only");
                        results_sig.set(sp_tracks);
                        playlists_sig.set(merge_playlist_hits(sp_playlists, Vec::new()));
                    }
                    (Some(sc_e), Some(sp_e)) => {
                        error_sig.set(Some(format!(
                            "both providers failed — sc: {sc_e}; spotify: {sp_e}"
                        )));
                        // Don't leave the previous query's results behind
                        // an error banner where Enter could still play them.
                        results_sig.set(Vec::new());
                        playlists_sig.set(Vec::new());
                    }
                }
                results_for_sig.set(snapshot.clone());
                is_searching_sig.set(false);
            });
        }
    });

    let search = UseSearch {
        query,
        results,
        artists,
        playlists,
        is_searching,
        error,
        results_for,
    };
    use_context_provider(move || search);
}

pub fn use_search() -> UseSearch {
    use_context::<UseSearch>()
}

/// Artist hits: Spotify first (there's a full profile behind the click),
/// SC users only for names Spotify didn't cover.
fn merge_artist_hits(spotify: Vec<Artist>, soundcloud: Vec<Artist>) -> Vec<Artist> {
    let mut merged = spotify;
    let mut seen: HashSet<String> = merged.iter().map(|a| match_key(&a.name)).collect();
    for a in soundcloud {
        if seen.insert(match_key(&a.name)) {
            merged.push(a);
        }
    }
    merged.truncate(8);
    merged
}

fn interleave<T>(a: Vec<T>, b: Vec<T>) -> Vec<T> {
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

fn merge_playlist_hits(
    spotify: Vec<PlaylistBrief>,
    soundcloud: Vec<PlaylistBrief>,
) -> Vec<PlaylistBrief> {
    let mut merged = interleave(spotify, soundcloud);
    merged.truncate(8);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_api::{
        ArtistUri, PlaylistBrief, PlaylistKind, PlaylistOpen, PlaylistUri, ProviderId,
    };

    fn artist(provider: ProviderId, name: &str) -> Artist {
        Artist {
            uri: ArtistUri(format!("{}:{}", provider.label().to_lowercase(), name)),
            provider,
            name: name.into(),
            image_url: None,
            genres: Vec::new(),
            permalink_url: None,
        }
    }

    #[test]
    fn spotify_wins_the_artist_dedupe() {
        let sp = vec![artist(ProviderId::Spotify, "goreshit")];
        let sc = vec![
            artist(ProviderId::SoundCloud, "GORESHIT"), // dupe by normalized name
            artist(ProviderId::SoundCloud, "sc only"),
        ];
        let merged = merge_artist_hits(sp, sc);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].provider, ProviderId::Spotify);
        assert_eq!(merged[1].name, "sc only");
    }

    fn playlist(provider: ProviderId, id: usize) -> PlaylistBrief {
        PlaylistBrief {
            uri: PlaylistUri(format!("{}:playlist:{id}", provider.label().to_lowercase())),
            provider,
            title: "Same title".into(),
            owner_name: None,
            cover_url: None,
            track_count: None,
            kind: PlaylistKind::User,
            open: PlaylistOpen::InApp,
        }
    }

    #[test]
    fn playlist_hits_alternate_preserve_titles_and_cap_at_eight() {
        let spotify = (0..5).map(|id| playlist(ProviderId::Spotify, id)).collect();
        let soundcloud = (0..5)
            .map(|id| playlist(ProviderId::SoundCloud, id))
            .collect();
        let merged = merge_playlist_hits(spotify, soundcloud);

        assert_eq!(merged.len(), 8);
        assert!(merged.iter().all(|playlist| playlist.title == "Same title"));
        for (index, playlist) in merged.iter().enumerate() {
            let expected = if index % 2 == 0 {
                ProviderId::Spotify
            } else {
                ProviderId::SoundCloud
            };
            assert_eq!(playlist.provider, expected);
        }
    }
}
