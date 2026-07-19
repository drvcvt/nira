//! Local play-log for the Home dashboard.
//!
//! JSON Lines at a caller-supplied path (typically
//! `~/.cache/nira/history.jsonl`), capped at 500 entries. Every mutation
//! serialises the full in-memory snapshot and enqueues it on the global
//! ordered persistence FIFO — the committing playback thread never touches
//! the disk, and mutation order == on-disk order like every other state
//! file. All recording is best-effort — IO failures log a warning and don't
//! propagate back to the playback flow, since "we couldn't record this for
//! Home" should never break audio.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const HISTORY_CAP: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub title: String,
    pub artist: String,
    /// Provider label as it appears in `ProviderId::label()` —
    /// "Spotify" / "SoundCloud" / "Local".
    pub provider: String,
    /// Exact provider track URI when known. Older history rows won't have it;
    /// Home falls back to text search for those.
    #[serde(default)]
    pub track_uri: Option<String>,
    #[serde(default)]
    pub cover_url: Option<String>,
    pub played_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct History {
    path: Option<PathBuf>,
    /// Newest at the end. We read this back front-to-back for `recent`.
    entries: Arc<Mutex<Vec<HistoryEntry>>>,
}

impl History {
    /// Construct a history backed by `path`. If `path` is None, the log lives
    /// only in memory — useful for tests and for the rare boot path where
    /// no cache dir resolved.
    pub fn open(path: Option<PathBuf>) -> Self {
        let entries = path.as_deref().map(load_jsonl).unwrap_or_default();
        Self {
            path,
            entries: Arc::new(Mutex::new(entries)),
        }
    }

    /// Append one entry. Deduplicates against the immediate previous entry
    /// so a `play_bytes` + `play_bytes` repeat doesn't fill the log; the
    /// scrobble watcher already gates per-track over a longer window, so we
    /// only need the trivial last-equal guard here.
    pub fn record(&self, entry: HistoryEntry) {
        let snapshot = {
            let mut entries = self.entries.lock().unwrap();
            if entries
                .last()
                .is_some_and(|last| last.title == entry.title && last.artist == entry.artist)
            {
                return;
            }
            entries.push(entry);
            if entries.len() > HISTORY_CAP {
                let overflow = entries.len() - HISTORY_CAP;
                entries.drain(..overflow);
            }
            entries.clone()
        };
        self.persist(&snapshot);
    }

    /// Serialise the snapshot (cheap — ≤500 short lines) and enqueue the
    /// disk write on the ordered persistence FIFO.
    fn persist(&self, entries: &[HistoryEntry]) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let mut buf = String::new();
        for e in entries {
            if let Ok(s) = serde_json::to_string(e) {
                buf.push_str(&s);
                buf.push('\n');
            }
        }
        if let Err(e) = config::AppConfig::atomic_write_bg(path, buf.into_bytes()) {
            tracing::warn!(error = %e, "history: persist failed");
        }
    }

    /// Newest first, capped at `n`. Returns owned copies so callers can pass
    /// them across thread boundaries without holding the internal lock.
    pub fn recent(&self, n: usize) -> Vec<HistoryEntry> {
        let entries = self.entries.lock().unwrap();
        entries.iter().rev().take(n).cloned().collect()
    }

    /// Remove one entry, identified by timestamp + title (played_at is
    /// unique enough — two same-second plays of the same title are
    /// indistinguishable to the user anyway). Rewrites the file.
    pub fn remove(&self, played_at: DateTime<Utc>, title: &str) {
        let snapshot = {
            let mut entries = self.entries.lock().unwrap();
            let before = entries.len();
            entries.retain(|e| !(e.played_at == played_at && e.title == title));
            if entries.len() == before {
                return;
            }
            entries.clone()
        };
        self.persist(&snapshot);
    }

    /// Clear both the in-memory buffer and the persisted JSONL file. The
    /// delete rides the same FIFO as the writes, so a still-queued snapshot
    /// can't resurrect the file afterwards.
    pub fn clear(&self) -> std::io::Result<()> {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
        if let Some(path) = self.path.clone() {
            config::AppConfig::remove_bg(path).map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

fn load_jsonl(path: &Path) -> Vec<HistoryEntry> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<HistoryEntry>(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, uri: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            title: title.to_string(),
            artist: "Artist".to_string(),
            provider: "SoundCloud".to_string(),
            track_uri: uri.map(str::to_string),
            cover_url: None,
            played_at: Utc::now(),
        }
    }

    #[test]
    fn records_exact_track_uri_for_replay() {
        let history = History::open(None);
        history.record(entry("Track", Some("soundcloud:track:123")));
        let recent = history.recent(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].track_uri.as_deref(), Some("soundcloud:track:123"));
    }

    #[test]
    fn dedupes_immediate_repeats() {
        let history = History::open(None);
        history.record(entry("Track", Some("soundcloud:track:123")));
        history.record(entry("Track", Some("soundcloud:track:123")));
        assert_eq!(history.recent(10).len(), 1);
    }
}
