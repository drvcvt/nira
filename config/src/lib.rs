//! Persisted user settings.
//!
//! Lives behind a `load`/`save` pair so the rest of the app can treat config
//! as a plain struct and not worry about IO or directory resolution.

use std::path::{Path, PathBuf};
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
    Write(PathBuf, Vec<u8>, Option<u32>),
    WriteConfirmed(
        PathBuf,
        Vec<u8>,
        Option<u32>,
        std::sync::mpsc::SyncSender<Result<(), String>>,
    ),
    Remove(PathBuf),
    /// Rendezvous marker: answered once everything enqueued before it hit
    /// the disk. Used by the atexit drain and tests.
    Flush(std::sync::mpsc::SyncSender<()>),
}

/// Completion handle for a write already ordered in the persistence queue.
#[must_use = "wait for the receipt to learn whether the write reached disk"]
pub struct PersistReceipt(std::sync::mpsc::Receiver<Result<(), String>>);

impl PersistReceipt {
    pub fn wait(self, timeout: std::time::Duration) -> anyhow::Result<()> {
        match self.0.recv_timeout(timeout) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(anyhow::anyhow!(error)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err(anyhow::anyhow!("save confirmation timed out"))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(anyhow::anyhow!("persistence writer thread gone"))
            }
        }
    }
}

fn persist_tx() -> &'static std::sync::mpsc::Sender<PersistJob> {
    PERSIST_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<PersistJob>();
        std::thread::Builder::new()
            .name("nira-persist".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        PersistJob::Write(path, bytes, mode) => {
                            if let Err(error) = AppConfig::atomic_write_mode(&path, &bytes, mode) {
                                tracing::warn!(%error, path = %path.display(), "background persist failed");
                            }
                        }
                        PersistJob::WriteConfirmed(path, bytes, mode, done) => {
                            let result = AppConfig::atomic_write_mode(&path, &bytes, mode)
                                .map_err(|error| error.to_string());
                            let _ = done.send(result);
                        }
                        PersistJob::Remove(path) => {
                            if let Err(error) = AppConfig::remove(&path) {
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

pub enum JsonLoad<T> {
    Missing,
    Loaded(T),
    Quarantined { backup: PathBuf, reason: String },
    Blocked { reason: String },
}

/// Read durable JSON without ever turning malformed bytes into an empty
/// replacement. Invalid files are hard-linked to a unique sibling before
/// the original name is removed; if either step fails, callers must disable
/// persistence for that path.
pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> JsonLoad<T> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return JsonLoad::Missing,
        Err(error) => {
            let reason = format!("could not read {}: {error}", path.display());
            tracing::warn!(%reason, "durable JSON load blocked");
            return JsonLoad::Blocked { reason };
        }
    };
    match serde_json::from_slice(&raw) {
        Ok(value) => JsonLoad::Loaded(value),
        Err(error) => {
            let parse_reason = error.to_string();
            match quarantine_invalid_json(path, &raw) {
                Ok(backup) => {
                    tracing::warn!(
                        path = %path.display(),
                        backup = %backup.display(),
                        reason = %parse_reason,
                        "invalid JSON preserved before recovery"
                    );
                    JsonLoad::Quarantined {
                        backup,
                        reason: parse_reason,
                    }
                }
                Err(quarantine_error) => {
                    let reason = format!(
                        "invalid JSON at {} ({parse_reason}); preservation failed: {quarantine_error}",
                        path.display()
                    );
                    tracing::warn!(%reason, "durable JSON load blocked");
                    JsonLoad::Blocked { reason }
                }
            }
        }
    }
}

fn quarantine_invalid_json(path: &Path, expected: &[u8]) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    loop {
        let backup = parent.join(format!(
            ".{name}.corrupt-{seconds}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::hard_link(path, &backup) {
            Ok(()) => {
                match std::fs::read(&backup) {
                    Ok(actual) if actual == expected => {}
                    Ok(_) => {
                        let _ = std::fs::remove_file(&backup);
                        return Err("backup verification did not match original bytes".into());
                    }
                    Err(error) => {
                        let _ = std::fs::remove_file(&backup);
                        return Err(format!("could not verify {}: {error}", backup.display()));
                    }
                }
                if let Err(error) = std::fs::remove_file(path) {
                    let _ = std::fs::remove_file(&backup);
                    return Err(format!(
                        "could not retire invalid original {}: {error}",
                        path.display()
                    ));
                }
                return Ok(backup);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not preserve {} beside the original: {error}",
                    path.display()
                ));
            }
        }
    }
}

fn persist_confirmed(
    path: PathBuf,
    bytes: Vec<u8>,
    mode: Option<u32>,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    persist_confirmed_bg(path, bytes, mode)?.wait(timeout)
}

fn persist_confirmed_bg(
    path: PathBuf,
    bytes: Vec<u8>,
    mode: Option<u32>,
) -> anyhow::Result<PersistReceipt> {
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    persist_tx()
        .send(PersistJob::WriteConfirmed(path, bytes, mode, done_tx))
        .map_err(|_| anyhow::anyhow!("persistence writer thread gone"))?;
    Ok(PersistReceipt(done_rx))
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


    /// 0.0 – 1.0 linear gain. The audio engine applies a square-law taper
    /// before feeding the output stream so the slider feels even.
    #[serde(default = "default_volume")]
    pub volume: f32,

    /// Lightweight three-band equalizer: low, mid and high gains in dB.
    #[serde(default)]
    pub equalizer_enabled: bool,
    #[serde(default)]
    pub equalizer_bands: [f32; 3],

    /// User-registered Spotify Developer app Client ID. Required for the
    /// OAuth PKCE handshake. Set via Settings → Spotify. Empty until the user
    /// pastes one in.
    #[serde(default)]
    pub spotify_client_id: Option<String>,

    /// Optional public SoundCloud profile used as the default playlist
    /// import source. Arbitrary public playlist links can still be pasted.
    #[serde(default)]
    pub soundcloud_profile_url: Option<String>,

    /// Share provider-blind now-playing metadata through Discord Rich Presence.
    #[serde(default = "default_true")]
    pub discord_presence: bool,

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

    #[serde(skip)]
    persistence_blocked: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            library_root: None,
            theme: ThemePref::default(),
            ui_font: None,
            volume: default_volume(),
            equalizer_enabled: false,
            equalizer_bands: [0.0; 3],
            spotify_client_id: None,
            soundcloud_profile_url: None,
            discord_presence: true,
            listenbrainz_token: None,
            listenbrainz_username: None,
            lastfm_api_key: None,
            discovery_soundcloud: true,
            discovery_listenbrainz: false,
            discovery_lastfm: true,
            persistence_blocked: false,
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

    /// Rebuildable provider cache. On a cache
    /// wipe we re-scrape and re-login from the config email/password.

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
            .send(PersistJob::Write(path, bytes, None))
            .map_err(|_| anyhow::anyhow!("persistence writer thread gone"))
    }

    /// [`Self::atomic_write_bg`] for secret-bearing files (tokens, auth
    /// caches): the file lands with 0600 perms on every write — a one-time
    /// chmod would not survive the rename-replace of the next save.
    pub fn atomic_write_secret_bg(path: std::path::PathBuf, bytes: Vec<u8>) -> anyhow::Result<()> {
        persist_tx()
            .send(PersistJob::Write(path, bytes, Some(0o600)))
            .map_err(|_| anyhow::anyhow!("persistence writer thread gone"))
    }

    /// Synchronous secret write — 0600 like [`Self::atomic_write_secret_bg`].
    pub fn atomic_write_secret_json<T: serde::Serialize>(
        path: &std::path::Path,
        value: &T,
    ) -> anyhow::Result<()> {
        Self::atomic_write_mode(path, &serde_json::to_vec(value)?, Some(0o600))
    }

    /// Ordered delete: runs behind every already-enqueued write, so a
    /// pending write to the same path can't resurrect the file afterwards.
    pub fn remove_bg(path: std::path::PathBuf) -> anyhow::Result<()> {
        persist_tx()
            .send(PersistJob::Remove(path))
            .map_err(|_| anyhow::anyhow!("persistence writer thread gone"))
    }

    /// Delete a file and durably commit the directory entry change.
    pub fn remove(path: &std::path::Path) -> anyhow::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => sync_parent(path).map_err(Into::into),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
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

    /// Enqueue JSON without blocking the caller and return its disk result.
    pub fn atomic_write_json_confirmed_bg<T: serde::Serialize>(
        path: std::path::PathBuf,
        value: &T,
    ) -> anyhow::Result<PersistReceipt> {
        persist_confirmed_bg(path, serde_json::to_vec(value)?, None)
    }

    /// Background variant of [`Self::save`] for hot paths (volume drags).
    /// Secret-tier: config.json carries the ListenBrainz/Last.fm
    /// tokens, so it gets the same 0600 treatment as the auth caches.
    pub fn save_bg(&self) -> anyhow::Result<()> {
        if self.persistence_blocked {
            anyhow::bail!("config persistence is disabled because the existing file is unreadable");
        }
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        Self::atomic_write_secret_bg(path, serde_json::to_vec_pretty(self)?)
    }

    pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
        Self::atomic_write_mode(path, bytes, None)
    }

    /// `mode` (Unix permission bits) is applied to the temp file *before* the
    /// rename, so the secret is never world-readable even for an instant.
    fn atomic_write_mode(
        path: &std::path::Path,
        bytes: &[u8],
        mode: Option<u32>,
    ) -> anyhow::Result<()> {
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
        let result = write_synced(&tmp, bytes, mode)
            .and_then(|()| std::fs::rename(&tmp, path))
            .and_then(|()| sync_parent(path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(Into::into)
    }

    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = Self::config_path() else {
            return Ok(Self::default());
        };
        // Retro-tighten files written before saves went 0600; the write
        // path keeps them that way from here on.
        if path.is_file() {
            tighten_secret_perms(&path);
        }
        Ok(match load_json(&path) {
            JsonLoad::Loaded(config) => config,
            JsonLoad::Missing | JsonLoad::Quarantined { .. } => Self::default(),
            JsonLoad::Blocked { .. } => Self {
                persistence_blocked: true,
                ..Self::default()
            },
        })
    }

    /// Queued like every other state write: a synchronous bypass here could
    /// be overtaken by an older `save_bg` snapshot still sitting in the
    /// writer queue (volume drag → Settings change → boot reverts it).
    /// Low-frequency callers wait for this exact FIFO job, so success means
    /// the bytes reached disk rather than merely reaching the queue.
    pub fn save(&self) -> anyhow::Result<()> {
        if self.persistence_blocked {
            anyhow::bail!("config persistence is disabled because the existing file is unreadable");
        }
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        persist_confirmed(
            path,
            serde_json::to_vec_pretty(self)?,
            Some(0o600),
            std::time::Duration::from_secs(10),
        )
    }
}

/// Write the temp file, apply the optional mode, and fsync before the
/// caller renames it into place. Without the fsync, a power loss shortly
/// after the rename could land the *rename* on disk but not the *data* —
/// a zero-length file where state used to be.
fn write_synced(tmp: &std::path::Path, bytes: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(tmp)?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    f.write_all(bytes)?;
    f.sync_all()
}

fn sync_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Remove crash-orphaned atomic-write temp files (`.<name>.tmp-<pid>-<n>`)
/// from the config and cache dir roots. Anything not written by the
/// current process is leftover from a crashed/killed instance — its rename
/// never happened, so it's garbage. Call once at boot.
pub fn sweep_stale_tmp_files() {
    for dir in [AppConfig::config_dir(), AppConfig::cache_dir()]
        .into_iter()
        .flatten()
    {
        sweep_stale_tmp_in(&dir, std::process::id());
    }
}

fn sweep_stale_tmp_in(dir: &std::path::Path, own_pid: u32) {
    let own = own_pid.to_string();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let name_os = entry.file_name();
        let Some(name) = name_os.to_str() else {
            continue;
        };
        let Some(after) = name
            .strip_prefix('.')
            .and_then(|n| n.rsplit_once(".tmp-"))
            .map(|(_, after)| after)
        else {
            continue;
        };
        // `after` is "<pid>-<n>"; skip our own in-flight writes.
        if after.split('-').next() == Some(own.as_str()) {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            tracing::info!(file = name, "removed orphaned temp file");
        }
    }
}

/// Best-effort chmod 0600 for an existing secret file (no-op if absent or
/// on non-Unix). Boot-time repair for files created before writes carried
/// modes; new writes come out 0600 already.
pub fn tighten_secret_perms(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
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
    fn missing_soundcloud_profile_url_defaults_to_none() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.soundcloud_profile_url, None);
    }

    #[test]
    fn discovery_sources_default_to_aegis_style() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.discovery_soundcloud);
        assert!(!cfg.discovery_listenbrainz);
        assert!(cfg.discovery_lastfm);
    }

    #[test]
    fn discord_presence_defaults_on_for_existing_configs() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.discord_presence);
    }

    #[test]
    fn missing_equalizer_config_defaults_flat_and_off() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.equalizer_enabled);
        assert_eq!(cfg.equalizer_bands, [0.0; 3]);
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
    fn sweep_removes_foreign_tmp_keeps_own_and_real_files() {
        let dir = temp_dir("tmp-sweep");
        std::fs::create_dir_all(&dir).unwrap();
        let own_pid = std::process::id();
        let foreign = dir.join(".state.json.tmp-99999-0");
        let own = dir.join(format!(".state.json.tmp-{own_pid}-0"));
        let real = dir.join("state.json");
        for p in [&foreign, &own, &real] {
            std::fs::write(p, b"{}").unwrap();
        }

        sweep_stale_tmp_in(&dir, own_pid);

        assert!(!foreign.exists(), "foreign tmp should be swept");
        assert!(own.exists(), "own in-flight tmp must survive");
        assert!(real.exists(), "real state file must survive");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn secret_writes_land_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("secret-mode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.json");

        AppConfig::atomic_write_secret_json(&path, &serde_json::json!({ "token": "x" })).unwrap();
        let sync_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(sync_mode, 0o600);

        AppConfig::atomic_write_secret_bg(path.clone(), b"{\"token\":\"y\"}".to_vec()).unwrap();
        AppConfig::flush_persist_queue(std::time::Duration::from_secs(10));
        let bg_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(bg_mode, 0o600);

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

    #[test]
    fn missing_json_is_reported_without_creating_a_file() {
        let dir = temp_dir("json-missing");
        let path = dir.join("state.json");

        assert!(matches!(
            load_json::<serde_json::Value>(&path),
            JsonLoad::Missing
        ));
        assert!(!path.exists());
    }

    #[test]
    fn valid_json_loads_normally() {
        let dir = temp_dir("json-valid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, br#"{"version":2}"#).unwrap();

        let JsonLoad::Loaded(value) = load_json::<serde_json::Value>(&path) else {
            panic!("valid JSON was not loaded");
        };
        assert_eq!(value["version"], 2);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_json_is_quarantined_byte_for_byte() {
        let dir = temp_dir("json-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let invalid = b"{ definitely not json";
        std::fs::write(&path, invalid).unwrap();

        let JsonLoad::Quarantined { backup, .. } = load_json::<serde_json::Value>(&path) else {
            panic!("malformed JSON was not quarantined");
        };
        assert!(!path.exists());
        assert_eq!(std::fs::read(&backup).unwrap(), invalid);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_json_is_blocked_without_touching_the_path() {
        let dir = temp_dir("json-unreadable");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::create_dir(&path).unwrap();

        assert!(matches!(
            load_json::<serde_json::Value>(&path),
            JsonLoad::Blocked { .. }
        ));
        assert!(path.is_dir());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn confirmed_background_write_returns_the_disk_error() {
        let dir = temp_dir("confirmed-error");
        std::fs::create_dir_all(&dir).unwrap();
        let parent_is_file = dir.join("not-a-directory");
        std::fs::write(&parent_is_file, b"keep").unwrap();
        let target = parent_is_file.join("config.json");

        let receipt = AppConfig::atomic_write_json_confirmed_bg(
            target,
            &serde_json::json!({ "version": 1 }),
        )
        .unwrap();
        let error = receipt
            .wait(std::time::Duration::from_secs(10))
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(std::fs::read(&parent_is_file).unwrap(), b"keep");

        std::fs::remove_dir_all(dir).unwrap();
    }
}
