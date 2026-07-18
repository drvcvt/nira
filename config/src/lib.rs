//! Persisted user settings.
//!
//! Lives behind a `load`/`save` pair so the rest of the app can treat config
//! as a plain struct and not worry about IO or directory resolution.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_ID: AtomicU64 = AtomicU64::new(0);
/// Sender into the single background persistence writer (see
/// [`AppConfig::atomic_write_json_bg`]); lazily spawned on first use.
static PERSIST_TX: std::sync::OnceLock<std::sync::mpsc::Sender<PersistJob>> =
    std::sync::OnceLock::new();

/// Work items for the single persistence writer. Every state-file mutation
/// (writes AND deletes) goes through this FIFO so mutation order == on-disk
/// order; a delete enqueued after a write can never be undone by it.
enum PersistJob {
    Write(PathBuf, Vec<u8>),
    Remove(PathBuf),
    /// Rendezvous marker: answered once everything enqueued before it hit
    /// the disk. Used by the atexit drain and tests.
    Flush(std::sync::mpsc::SyncSender<()>),
}

fn persist_tx() -> &'static std::sync::mpsc::Sender<PersistJob> {
    PERSIST_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<PersistJob>();
        std::thread::Builder::new()
            .name("nira-persist".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        PersistJob::Write(path, bytes) => {
                            if let Err(error) = AppConfig::atomic_write(&path, &bytes) {
                                tracing::warn!(%error, path = %path.display(), "background persist failed");
                            }
                        }
                        PersistJob::Remove(path) => {
                            if let Err(error) = std::fs::remove_file(&path)
                                && error.kind() != std::io::ErrorKind::NotFound
                            {
                                tracing::warn!(%error, path = %path.display(), "background remove failed");
                            }
                        }
                        PersistJob::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("spawn persistence writer thread");
        // The desktop event loop leaves through `process::exit` (tao never
        // unwinds back into main), so a C atexit handler is the only hook
        // that still runs — without this drain, enqueued likes/playlist/
        // config writes die with the process on window close.
        unsafe {
            libc::atexit(drain_persist_queue_at_exit);
        }
        tx
    })
}

extern "C" fn drain_persist_queue_at_exit() {
    AppConfig::flush_persist_queue(std::time::Duration::from_secs(5));
}

/// UI theme preference. `System` defers to the OS/portal colour scheme via
/// CSS `prefers-color-scheme`; the explicit variants pin `data-theme` on the
/// document root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Where the user's local music collection lives. Empty until set.
    pub library_root: Option<PathBuf>,

    /// Light/dark/system theme choice for the shell.
    #[serde(default)]
    pub theme: ThemePref,

    /// UI font key (see [`ui_font_stack`]). None/unknown = bundled Geist.
    #[serde(default)]
    pub ui_font: Option<String>,

    /// the hi-res provider `user_auth_token` from the logged-in the provider web player session.
    /// the hi-res provider disabled email/password login server-side (April 2026), so token
    /// auth is the only working path — the user pastes their token from the
    /// browser. Stored plaintext like the other provider secrets. Empty =
    /// the hi-res provider disabled.
    #[serde(default)]
    pub hires-provider_token: Option<String>,
    /// Download/stream quality as a the hi-res provider `format_id`: 5=MP3, 6=CD FLAC,
    /// 7=24/≤96, 27=24/≤192. None = best (27).
    #[serde(default)]
    pub hires-provider_format_id: Option<u32>,

    /// 0.0 – 1.0 linear gain. The audio engine applies a square-law taper
    /// before feeding the output stream so the slider feels even.
    #[serde(default = "default_volume")]
    pub volume: f32,

    /// User-registered Spotify Developer app Client ID. Required for the
    /// OAuth PKCE handshake. Set via Settings → Spotify. Empty until the user
    /// pastes one in.
    #[serde(default)]
    pub spotify_client_id: Option<String>,

    /// User-supplied ListenBrainz token (https://listenbrainz.org/profile/).
    /// Enables outbound scrobbling. Empty = scrobbling disabled.
    #[serde(default)]
    pub listenbrainz_token: Option<String>,

    /// ListenBrainz username. Needed *separately* from the token because the
    /// `/1/user/<name>/listens` path is keyed on the username and the token
    /// API doesn't expose whose token it is. Empty = Home's "Listened lately"
    /// row stays in its empty state.
    #[serde(default)]
    pub listenbrainz_username: Option<String>,

    /// Last.fm app-owned API key. Optional — if absent the discovery engine
    /// silently skips the Last.fm candidate source. Falls back to the
    /// `NIRA_LASTFM_API_KEY` env var at startup if this field is empty.
    #[serde(default)]
    pub lastfm_api_key: Option<String>,

    /// Discovery/Radio candidate sources. Defaults keep the Aegis-style
    /// SoundCloud-first feel: SC related + Last.fm, ListenBrainz opt-in.
    #[serde(default = "default_true")]
    pub discovery_soundcloud: bool,
    #[serde(default)]
    pub discovery_listenbrainz: bool,
    #[serde(default = "default_true")]
    pub discovery_lastfm: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            library_root: None,
            theme: ThemePref::default(),
            ui_font: None,
            hires-provider_token: None,
            hires-provider_format_id: None,
            volume: default_volume(),
            spotify_client_id: None,
            listenbrainz_token: None,
            listenbrainz_username: None,
            lastfm_api_key: None,
            discovery_soundcloud: true,
            discovery_listenbrainz: false,
            discovery_lastfm: true,
        }
    }
}

/// Map a stored UI-font key to its CSS font-family stack. Single source of
/// truth for the shell (applies the `--font-ui` variable) and the Settings
/// font picker. Unknown/None falls back to the bundled Geist.
pub fn ui_font_stack(key: Option<&str>) -> &'static str {
    match key {
        Some("inter") => r#""Inter", system-ui, sans-serif"#,
        Some("adwaita") => r#""Adwaita Sans", "Inter", system-ui, sans-serif"#,
        Some("noto") => r#""Noto Sans", system-ui, sans-serif"#,
        Some("system") => "system-ui, sans-serif",
        _ => r#""Geist", "Inter", system-ui, -apple-system, "Segoe UI", sans-serif"#,
    }
}

/// The pickable UI fonts: (key, display label). Order = display order.
pub const UI_FONTS: &[(&str, &str)] = &[
    ("geist", "Geist"),
    ("inter", "Inter"),
    ("adwaita", "Adwaita Sans"),
    ("noto", "Noto Sans"),
    ("system", "System"),
];

fn default_volume() -> f32 {
    0.8
}

fn default_true() -> bool {
    true
}

impl AppConfig {
    pub fn config_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "nira", "nira").map(|d| d.config_dir().to_path_buf())
    }

    /// XDG cache root for nira. Lower-trust state goes here — everything in
    /// `cache_dir()` is safe to nuke; the app rebuilds it on next launch.
    pub fn cache_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "nira", "nira").map(|d| d.cache_dir().to_path_buf())
    }

    pub fn config_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("config.json"))
    }

    /// Where the OAuth token bundle lives. Plain JSON with 0600 perms —
    /// good enough for a personal music app; real keyring integration is a
    /// later Phase if/when policy demands it.
    /// Local liked-songs store. Lives in the config dir (not cache)
    /// because the user expects deleting cache to be safe — losing
    /// hand-curated likes is not.
    pub fn likes_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("likes.json"))
    }

    pub fn spotify_tokens_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("spotify-tokens.json"))
    }

    pub fn spotify_liked_cache_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("spotify-liked.json"))
    }

    pub fn enrichment_cache_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("enrichment-cache.json"))
    }

    /// Directory for album art extracted from local files' tags. The scanner
    /// writes images here; the desktop shell serves them to the webview via
    /// the "/covers/…" asset handler.
    pub fn covers_cache_dir() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("covers"))
    }

    pub fn soundcloud_client_id_cache_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("soundcloud-client-id.json"))
    }

    /// Cached the hi-res provider app credentials + user auth token. Rebuildable: on a cache
    /// wipe we re-scrape and re-login from the config email/password.
    pub fn hires-provider_auth_cache_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("hires-provider-auth.json"))
    }

    /// Local play-log consumed by the Home page's "Recently played" row.
    pub fn play_history_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("history.jsonl"))
    }

    /// Persisted For-You shelves/mixes/tiles. Lets Home show the previous
    /// dashboard on cold start instead of an empty wait state.
    pub fn recommendations_cache_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("recommendations.json"))
    }

    /// Persisted playback queue (entries + index + modes) so a restart
    /// resumes where the session left off. Cache-tier: losing it just means
    /// an empty queue on next boot.
    pub fn queue_state_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("queue.json"))
    }

    /// Local playlists. Config-tier like likes — hand-curated lists must
    /// survive a cache wipe.
    pub fn playlists_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("playlists.json"))
    }

    /// Last playback position (uri + seconds), written every few seconds
    /// while playing so a restart can resume mid-track.
    pub fn playback_position_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("position.json"))
    }

    /// Atomic write helper: serialise → write to a unique temp file → rename. Used
    /// for every cache file we own so a kill-at-the-wrong-moment can't leave
    /// a half-written JSON that breaks the next launch.
    pub fn atomic_write_json<T: serde::Serialize>(
        path: &std::path::Path,
        value: &T,
    ) -> anyhow::Result<()> {
        let raw = serde_json::to_vec(value)?;
        Self::atomic_write(path, &raw)
    }

    /// Like [`Self::atomic_write`], but the disk write happens on ONE
    /// background writer thread. Serialisation stays on the caller, so the
    /// bytes are fixed at enqueue time and the single FIFO consumer keeps
    /// mutation order == persistence order — the same invariant as the
    /// synchronous path, minus the UI-thread disk stalls. Shutdown safety
    /// comes from the atexit drain in [`persist_tx`], not from bypassing
    /// the queue.
    pub fn atomic_write_bg(path: std::path::PathBuf, bytes: Vec<u8>) -> anyhow::Result<()> {
        persist_tx()
            .send(PersistJob::Write(path, bytes))
            .map_err(|_| anyhow::anyhow!("persistence writer thread gone"))
    }

    /// Ordered delete: runs behind every already-enqueued write, so a
    /// pending write to the same path can't resurrect the file afterwards.
    pub fn remove_bg(path: std::path::PathBuf) -> anyhow::Result<()> {
        persist_tx()
            .send(PersistJob::Remove(path))
            .map_err(|_| anyhow::anyhow!("persistence writer thread gone"))
    }

    /// Block until every job enqueued so far has been executed (bounded —
    /// flushing beats hanging process exit on a wedged disk).
    pub fn flush_persist_queue(timeout: std::time::Duration) {
        let Some(tx) = PERSIST_TX.get() else { return };
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        if tx.send(PersistJob::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv_timeout(timeout);
        }
    }

    /// JSON convenience over [`Self::atomic_write_bg`].
    pub fn atomic_write_json_bg<T: serde::Serialize>(
        path: std::path::PathBuf,
        value: &T,
    ) -> anyhow::Result<()> {
        Self::atomic_write_bg(path, serde_json::to_vec(value)?)
    }

    /// Background variant of [`Self::save`] for hot paths (volume drags).
    pub fn save_bg(&self) -> anyhow::Result<()> {
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        Self::atomic_write_bg(path, serde_json::to_vec_pretty(self)?)
    }

    pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
        // ponytail: one global lock keeps low-frequency persistence simple;
        // use per-path locks only if write contention becomes measurable.
        let _guard = WRITE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("persistence lock poisoned"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid persistence path: {}", path.display()))?;
        let tmp = path.with_file_name(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(Into::into)
    }

    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = Self::config_path() else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// Queued like every other state write: a synchronous bypass here could
    /// be overtaken by an older `save_bg` snapshot still sitting in the
    /// writer queue (volume drag → Settings change → boot reverts it).
    /// Disk errors surface in the writer's log; callers only see enqueue
    /// failures.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_bg()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nira-config-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn default_volume_is_sane() {
        assert_eq!(AppConfig::default().volume, 0.8);
    }

    #[test]
    fn missing_volume_deserialises_to_default() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.volume, 0.8);
    }

    #[test]
    fn discovery_sources_default_to_aegis_style() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.discovery_soundcloud);
        assert!(!cfg.discovery_listenbrainz);
        assert!(cfg.discovery_lastfm);
    }

    #[test]
    fn writer_queue_is_fifo_across_writes_removes_and_flush() {
        let dir = temp_dir("bg-order");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");

        AppConfig::atomic_write_bg(path.clone(), b"{\"v\":1}".to_vec()).unwrap();
        AppConfig::remove_bg(path.clone()).unwrap();
        AppConfig::atomic_write_bg(path.clone(), b"{\"v\":2}".to_vec()).unwrap();
        AppConfig::flush_persist_queue(std::time::Duration::from_secs(10));

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"v\":2}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_write_does_not_reuse_shared_temp_path() {
        let dir = temp_dir("atomic-write");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::create_dir(path.with_extension("tmp")).unwrap();

        AppConfig::atomic_write_json(&path, &serde_json::json!({ "version": 2 })).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);

        std::fs::remove_dir_all(dir).unwrap();
    }
}
