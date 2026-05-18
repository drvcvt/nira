//! Persisted user settings.
//!
//! Lives behind a `load`/`save` pair so the rest of the app can treat config
//! as a plain struct and not worry about IO or directory resolution.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Where the user's local music collection lives. Empty until set.
    pub library_root: Option<PathBuf>,

    /// 0.0 – 1.0 linear gain. The audio engine applies a square-law taper
    /// before feeding the output stream so the slider feels even.
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
}

impl AppConfig {
    pub fn config_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "nira", "nira")
            .map(|d| d.config_dir().to_path_buf())
    }

    /// XDG cache root for nira. Lower-trust state goes here — everything in
    /// `cache_dir()` is safe to nuke; the app rebuilds it on next launch.
    pub fn cache_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "nira", "nira")
            .map(|d| d.cache_dir().to_path_buf())
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

    pub fn soundcloud_client_id_cache_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("soundcloud-client-id.json"))
    }

    /// Local play-log consumed by the Home page's "Recently played" row.
    pub fn play_history_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("history.jsonl"))
    }

    /// Atomic write helper: serialise → write to `path.tmp` → rename. Used
    /// for every cache file we own so a kill-at-the-wrong-moment can't leave
    /// a half-written JSON that breaks the next launch.
    pub fn atomic_write_json<T: serde::Serialize>(
        path: &std::path::Path,
        value: &T,
    ) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        let raw = serde_json::to_string(value)?;
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
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

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw)?;
        Ok(())
    }
}
