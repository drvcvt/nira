//! Persisted user settings.
//!
//! Lives behind a `load`/`save` pair so the rest of the app can treat config
//! as a plain struct and not worry about IO or directory resolution.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
