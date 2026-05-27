//! Reactive surface for the user's library — Spotify "Liked Songs" with
//! smart-rescan disk caching. Playback dispatch is in `queue.rs`; pages
//! call `queue.play_list(liked, idx)`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use config::AppConfig;
use dioxus::prelude::*;
use provider_api::Track;
use provider_spotify::SpotifyProvider;
use serde::{Deserialize, Serialize};

const REFRESH_AFTER_SECS: u64 = 60 * 60 * 6; // 6 hours
const RETRY_AFTER_SECS: u64 = 60 * 15; // rate-limit/backoff guard
static LAST_SPOTIFY_REFRESH_ATTEMPT: AtomicU64 = AtomicU64::new(0);

/// Bump when the on-disk shape changes. v2 introduced `Track::added_at`;
/// older caches lose their `added_at` on read so Home's "Recently liked"
/// sort would collapse. Treat any non-matching version as no cache and
/// trigger a fresh paginated walk on next launch.
const CACHE_VERSION: u32 = 2;

/// How many items the Home "Recently liked" row shows.
const RECENTLY_LIKED_ROW: usize = 8;

#[derive(Clone, PartialEq)]
pub struct UseLibrary {
    pub liked: Signal<Vec<Track>>,
    /// Subset of `liked`, sorted by `Track::added_at` desc, capped at
    /// [`RECENTLY_LIKED_ROW`]. Memoised so re-rendering the Home row doesn't
    /// re-sort on every tick — only when `liked` itself changes.
    pub recently_liked: Memo<Vec<Track>>,
    pub is_loading: Signal<bool>,
    pub progress: Signal<(u32, u32)>,
    pub error: Signal<Option<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LikedDiskCache {
    #[serde(default)]
    version: u32,
    fetched_at_unix: u64,
    tracks: Vec<Track>,
}

fn read_disk_cache() -> Option<LikedDiskCache> {
    let path = AppConfig::spotify_liked_cache_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let cache: LikedDiskCache = serde_json::from_str(&raw).ok()?;
    if cache.version != CACHE_VERSION {
        return None;
    }
    Some(cache)
}

fn write_disk_cache(tracks: &[Track]) {
    let Some(path) = AppConfig::spotify_liked_cache_path() else {
        return;
    };
    let cache = LikedDiskCache {
        version: CACHE_VERSION,
        fetched_at_unix: now_unix(),
        tracks: tracks.to_vec(),
    };
    if let Err(e) = AppConfig::atomic_write_json(&path, &cache) {
        tracing::warn!(error = %e, "could not persist liked-tracks cache");
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn spotify_refresh_allowed(now: u64) -> bool {
    let last = LAST_SPOTIFY_REFRESH_ATTEMPT.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < RETRY_AFTER_SECS {
        return false;
    }
    LAST_SPOTIFY_REFRESH_ATTEMPT.store(now, Ordering::Relaxed);
    true
}

pub fn use_library() -> UseLibrary {
    let sp = use_context::<Arc<SpotifyProvider>>();

    let liked = use_signal(Vec::<Track>::new);
    let is_loading = use_signal(|| false);
    let progress = use_signal(|| (0u32, 0u32));
    let error = use_signal(|| None::<String>);

    use_hook({
        let sp = sp.clone();
        move || {
            let sp = sp.clone();
            let mut liked_sig = liked;
            let mut loading_sig = is_loading;
            let mut progress_sig = progress;
            let mut error_sig = error;

            let cache = read_disk_cache();
            let cached_len = cache.as_ref().map(|c| c.tracks.len()).unwrap_or(0);
            let has_cache = cached_len > 0;
            let cache_fresh = cache
                .as_ref()
                .map(|c| now_unix().saturating_sub(c.fetched_at_unix) < REFRESH_AFTER_SECS)
                .unwrap_or(false);
            if let Some(c) = cache.as_ref() {
                liked_sig.set(c.tracks.clone());
                progress_sig.set((c.tracks.len() as u32, c.tracks.len() as u32));
            }

            spawn(async move {
                if !sp.is_connected() {
                    if !has_cache {
                        error_sig.set(Some(
                            "Connect Spotify in Settings to see your Liked Songs.".into(),
                        ));
                    }
                    return;
                }
                if cache_fresh {
                    return;
                }
                if has_cache && !spotify_refresh_allowed(now_unix()) {
                    return;
                }

                let show_spinner = !has_cache;
                if show_spinner {
                    loading_sig.set(true);
                    error_sig.set(None);
                }

                let mut accumulator: Vec<Track> = Vec::new();
                let result = sp
                    .liked_tracks_all(|page_tracks, loaded, total| {
                        accumulator.extend(page_tracks);
                        if show_spinner {
                            liked_sig.set(accumulator.clone());
                        }
                        progress_sig.set((loaded, total));
                    })
                    .await;

                match result {
                    Ok(()) => {
                        liked_sig.set(accumulator.clone());
                        write_disk_cache(&accumulator);
                        LAST_SPOTIFY_REFRESH_ATTEMPT.store(0, Ordering::Relaxed);
                    }
                    Err(e) => {
                        if has_cache {
                            tracing::warn!(error = %e, "background liked-tracks refresh failed");
                        } else {
                            error_sig.set(Some(e.to_string()));
                        }
                    }
                }
                if show_spinner {
                    loading_sig.set(false);
                }
            });
        }
    });

    let recently_liked = use_memo(move || {
        let mut sorted: Vec<Track> = liked.read().clone();
        sorted.sort_by(|a, b| match (a.added_at, b.added_at) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        sorted.truncate(RECENTLY_LIKED_ROW);
        sorted
    });

    UseLibrary {
        liked,
        recently_liked,
        is_loading,
        progress,
        error,
    }
}
