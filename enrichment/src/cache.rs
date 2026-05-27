//! TTL cache for enrichment lookups.
//!
//! Disk-backed: loaded once at startup, written atomically on every `put`.
//! Cache file lives under `cache_dir()/enrichment-cache.json`. Entries past
//! their TTL are simply skipped on read and overwritten on the next miss; we
//! don't eagerly compact, so the file can grow a bit between launches, but
//! it's tiny in practice (~10KB after weeks of normal use).
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
        let map = self.inner.lock().ok()?;
        let entry = map.get(key)?;
        let now = now_unix();
        if now.saturating_sub(entry.inserted_at_unix) > self.ttl.as_secs() {
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

    /// Write the current map atomically. Called after each `put` — the cache
    /// is small enough (~10KB) that the cost is sub-millisecond and we get
    /// "survives a kill at any moment" for free.
    fn flush(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let snapshot = match self.inner.lock() {
            Ok(map) => OnDisk {
                entries: map.clone(),
            },
            Err(_) => return,
        };
        if let Err(e) = config::AppConfig::atomic_write_json(path, &snapshot) {
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
