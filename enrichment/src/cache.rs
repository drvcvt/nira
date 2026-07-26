//! TTL cache for enrichment lookups.
//!
//! Disk-backed: loaded once at startup, written atomically on every `put`
//! via the background persist FIFO. Cache file lives under
//! `cache_dir()/enrichment-cache.json`.
//!
//! Expired entries are dropped when the snapshot is built, so the file tracks
//! live entries rather than growing forever. It previously only skipped them
//! on read — since keys are per-recording they almost never recur, so the
//! file reached 200KB+ and every `put` rewrote and fsynced all of it on the
//! UI thread (a Home refresh is ~40 puts, i.e. ~8MB and 40 fsyncs per click,
//! scaling O(n²) in lookups).
//!
//! Keys are arbitrary strings namespaced by the caller (e.g.
//! `"mb:recording:Bowie|Heroes|3"`); values are caller-serialised JSON so the
//! cache stays agnostic of response shapes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// Unix seconds when the value was written. Compared against now-ttl on
    /// each read; expired entries report a cache miss.
    inserted_at_unix: u64,
    value: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct OnDisk {
    entries: HashMap<String, Entry>,
}

pub struct TtlCache {
    inner: Mutex<HashMap<String, Entry>>,
    ttl: Duration,
    path: Option<PathBuf>,
}

impl TtlCache {
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(60 * 60 * 24)) // 24 hours
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        let path = config::AppConfig::enrichment_cache_path();
        let entries = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<OnDisk>(&s).ok())
            .map(|d| d.entries)
            .unwrap_or_default();
        Self {
            inner: Mutex::new(entries),
            ttl,
            path,
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.get_fresher_than(key, self.ttl)
    }

    /// Like `get`, but with a caller-supplied maximum age (never longer than
    /// the cache-wide TTL). For data that must go stale faster than the
    /// default — e.g. the user's own listen feed, which otherwise looks
    /// frozen for a day.
    pub fn get_fresher_than(&self, key: &str, max_age: Duration) -> Option<String> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(key)?;
        let now = now_unix();
        let max_secs = max_age.as_secs().min(self.ttl.as_secs());
        if now.saturating_sub(entry.inserted_at_unix) > max_secs {
            None
        } else {
            Some(entry.value.clone())
        }
    }

    pub fn put(&self, key: String, value: String) {
        {
            let Ok(mut map) = self.inner.lock() else {
                return;
            };
            map.insert(
                key,
                Entry {
                    inserted_at_unix: now_unix(),
                    value,
                },
            );
        }
        self.flush();
    }

    /// Clear both the in-memory map and the persisted cache file.
    pub fn clear(&self) {
        if let Ok(mut map) = self.inner.lock() {
            map.clear();
        }
        if let Some(path) = self.path.as_ref()
            && let Err(e) = std::fs::remove_file(path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, "enrichment cache clear failed");
        }
    }

    /// Write the current map atomically. Called after each `put`, so it goes
    /// through the background persist FIFO (`_bg`) rather than the
    /// fsync-on-the-caller path — `put` is awaited from Dioxus tasks polled on
    /// the main thread, so the synchronous variant froze the UI.
    ///
    /// Expired entries are dropped here rather than only on read; that is the
    /// only thing keeping the file from growing without bound.
    fn flush(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let cutoff = now_unix().saturating_sub(self.ttl.as_secs());
        let snapshot = match self.inner.lock() {
            Ok(map) => OnDisk {
                entries: map
                    .iter()
                    .filter(|(_, e)| e.inserted_at_unix >= cutoff)
                    .map(|(k, e)| (k.clone(), e.clone()))
                    .collect(),
            },
            Err(_) => return,
        };
        if let Err(e) = config::AppConfig::atomic_write_json_bg(path.clone(), &snapshot) {
            tracing::warn!(error = %e, "enrichment cache flush failed");
        }
    }
}

impl Default for TtlCache {
    fn default() -> Self {
        Self::new()
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
