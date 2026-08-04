//! Spotify provider — OAuth PKCE + Web API.
//!
//! Phase 2A covers metadata only (search/track/artist). Audio playback
//! (`resolve_stream`) is deferred to Phase 2B when we wire librespot using
//! the OAuth access-token as the session credential.
//!
//! Auth model:
//! - User registers their own Spotify Developer app at developer.spotify.com
//!   and pastes the Client ID into Settings. Redirect URI in the app must be
//!   `http://127.0.0.1:7777/callback` (we hard-code that port for now).
//! - On Connect we run Authorization Code with PKCE: spin up a one-shot
//!   tokio TcpListener on 127.0.0.1:7777, open the browser, parse the
//!   `code=` query param off the redirect, exchange it for tokens.
//! - Tokens (access + refresh + expiry) persist as JSON at
//!   `~/.config/nira/spotify-tokens.json` with 0600 perms. Pragmatic —
//!   keyring crates on Linux are either non-persistent (keyutils) or pull in
//!   heavyweight dbus stacks; we'll revisit if/when policy needs it.
//! - Refresh happens on demand inside `access_token()` when within 60s of
//!   expiry. We rotate the refresh_token if Spotify returns a new one.

use std::path::PathBuf;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use provider_api::{
    AlbumBrief, AlbumDetail, AlbumRef, AlbumType, AlbumUri, Artist, ArtistRef, ArtistUri,
    PlaylistBrief, PlaylistKind, PlaylistOpen, PlaylistUri, Provider, ProviderCaps, ProviderError,
    ProviderId, ProviderResult, Query, RelatedArtist, SearchResults, StreamHandle, Track, TrackUri,
};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

const SP_API: &str = "https://api.spotify.com/v1";
const SP_AUTH: &str = "https://accounts.spotify.com/authorize";
const SP_TOKEN: &str = "https://accounts.spotify.com/api/token";
const CALLBACK_PORT: u16 = 7777;
const CALLBACK_PATH: &str = "/callback";
const SEARCH_MAX: u32 = 10;

/// Spotify OAuth scopes nira asks for. `streaming` is the librespot
/// prerequisite — we request it now even though Phase 2A doesn't use it, so
/// users don't have to re-consent when 2B lands.
const SCOPES: &[&str] = &[
    "user-read-private",
    "user-read-email",
    "user-library-read",
    "user-top-read",
    "user-read-recently-played",
    "playlist-read-private",
    "playlist-read-collaborative",
    "streaming",
];

pub struct SpotifyProvider {
    http: Client,
    client_id: Arc<StdRwLock<String>>,
    tokens_path: Option<PathBuf>,
    token: Arc<RwLock<Option<TokenSet>>>,
    /// Serializes token refreshes. Spotify rotates PKCE refresh tokens, so
    /// two concurrent refreshes invalidate each other — the loser would
    /// then delete the freshly-stored token and log the user out.
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenSet {
    access_token: String,
    refresh_token: Option<String>,
    expires_at_unix: u64,
}

#[derive(Deserialize)]
struct TokenResponseRaw {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl SpotifyProvider {
    /// Build a provider against `client_id`. Loads any persisted token from
    /// disk so a previous-session OAuth carries over.
    pub fn new(client_id: String, tokens_path: Option<PathBuf>) -> ProviderResult<Self> {
        let http = Client::builder()
            .user_agent("nira/0.1.0")
            // Fail instead of wedging: a half-open connection here used to
            // hang the token refresh_lock and with it every Spotify call.
            // read_timeout (not a total timeout) so slow-but-flowing large
            // responses survive.
            .connect_timeout(std::time::Duration::from_secs(10))
            .read_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(format!("http: {e}")))?;
        let token = tokens_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<TokenSet>(&s).ok());
        Ok(Self {
            http,
            client_id: Arc::new(StdRwLock::new(client_id)),
            tokens_path,
            token: Arc::new(RwLock::new(token)),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub fn client_id(&self) -> String {
        self.client_id
            .read()
            .map(|id| id.clone())
            .unwrap_or_default()
    }

    /// Update the active Spotify Developer Client ID without restarting.
    /// If it changes, existing OAuth tokens are cleared because Spotify
    /// refresh tokens are bound to the app/client they were issued for.
    pub async fn set_client_id(&self, client_id: String) -> ProviderResult<bool> {
        let next = client_id.trim().to_string();
        if self.client_id() == next {
            return Ok(false);
        }
        self.disconnect().await?;
        *self
            .client_id
            .write()
            .map_err(|_| ProviderError::Other("Spotify client_id lock poisoned".into()))? = next;
        Ok(true)
    }

    /// Has a token in memory (regardless of expiry — refresh handles that).
    pub fn is_connected(&self) -> bool {
        self.token.try_read().map(|t| t.is_some()).unwrap_or(false)
    }

    /// Fresh access token (refreshes if within 60s of expiry). Public so the
    /// `player` crate can hand it to its librespot backend.
    pub async fn access_token_for_playback(&self) -> ProviderResult<String> {
        self.access_token().await
    }

    /// Playback-path variant of the forced refresh: librespot's AP rejected
    /// `stale_access` even though it hasn't expired (typical after suspend).
    /// Same dedup semantics as the API-side 401 handling.
    pub async fn refresh_playback_token(&self, stale_access: &str) -> ProviderResult<String> {
        self.refresh_after_401(stale_access).await
    }

    /// Run the OAuth PKCE handshake. Blocks until the user completes the
    /// browser flow (or the listener times out / fails).
    pub async fn connect(&self) -> ProviderResult<()> {
        let client_id = self.client_id();
        if client_id.trim().is_empty() {
            return Err(ProviderError::Other(
                "Spotify client_id is not set — paste one in Settings first.".into(),
            ));
        }

        let redirect_uri = format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}");
        let (verifier, challenge) = generate_pkce_pair();
        let csrf_state: String = random_alphanum(32);

        // Build the auth URL. Standard Spotify Authorization Code w/ PKCE.
        let mut auth_url = format!(
            "{SP_AUTH}?response_type=code&client_id={cid}&redirect_uri={redir}&code_challenge_method=S256&code_challenge={ch}&state={state}",
            cid = urlencoded(&client_id),
            redir = urlencoded(&redirect_uri),
            ch = urlencoded(&challenge),
            state = urlencoded(&csrf_state),
        );
        if !SCOPES.is_empty() {
            auth_url.push_str("&scope=");
            auth_url.push_str(&urlencoded(&SCOPES.join(" ")));
        }

        // Bind the callback listener *before* opening the browser so the
        // redirect can't race ahead of us.
        let listener = TcpListener::bind(format!("127.0.0.1:{CALLBACK_PORT}"))
            .await
            .map_err(|e| {
                ProviderError::Other(format!(
                    "could not bind 127.0.0.1:{CALLBACK_PORT} for OAuth callback: {e}"
                ))
            })?;

        if let Err(e) = webbrowser::open(&auth_url) {
            tracing::warn!("webbrowser::open failed ({e}); printing URL instead");
            tracing::info!("Spotify auth URL: {auth_url}");
        }

        let (code, state) = wait_for_callback(listener).await?;
        if state != csrf_state {
            return Err(ProviderError::Other(
                "CSRF state mismatch in callback".into(),
            ));
        }

        // Exchange the code for tokens. PKCE means no client_secret.
        let resp = self
            .http
            .post(SP_TOKEN)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect_uri),
                ("client_id", &client_id),
                ("code_verifier", &verifier),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Other(format!(
                "token exchange failed ({status}): {body}"
            )));
        }
        let tr: TokenResponseRaw = resp
            .json()
            .await
            .map_err(|e| ProviderError::Malformed(e.to_string()))?;

        if self.client_id() != client_id {
            return Err(ProviderError::Other(
                "Spotify Client ID changed while OAuth was in progress — connect again.".into(),
            ));
        }

        let token = TokenSet {
            access_token: tr.access_token,
            refresh_token: tr.refresh_token,
            expires_at_unix: now_unix() + tr.expires_in.unwrap_or(3600),
        };
        self.persist_token(&token)?;
        *self.token.write().await = Some(token);
        tracing::info!("Spotify connected");
        Ok(())
    }

    pub async fn disconnect(&self) -> ProviderResult<()> {
        if let Some(p) = self.tokens_path.as_ref() {
            config::AppConfig::remove(p)
                .map_err(|e| ProviderError::Other(format!("token removal: {e}")))?;
        }
        *self.token.write().await = None;
        Ok(())
    }

    fn persist_token(&self, token: &TokenSet) -> ProviderResult<()> {
        let Some(path) = self.tokens_path.as_ref() else {
            return Ok(());
        };
        config::AppConfig::atomic_write_secret_json(path, token)
            .map_err(|e| ProviderError::Other(format!("token write: {e}")))
    }

    /// Get a fresh access token, refreshing if it's within 60s of expiry.
    /// Returns `AuthRequired` if no token has been obtained yet — UI uses
    /// that to prompt the user to Connect.
    async fn access_token(&self) -> ProviderResult<String> {
        let now = now_unix();
        let snapshot = self.token.read().await.clone();
        let token = snapshot.ok_or(ProviderError::AuthRequired)?;
        if now < token.expires_at_unix.saturating_sub(60) {
            return Ok(token.access_token);
        }

        // Refresh path — one task at a time. Concurrent API calls at expiry
        // are routine (artist page fires two fetches via join!), and Spotify
        // rotates the refresh token on use.
        let _refreshing = self.refresh_lock.lock().await;
        // Re-check under the lock: whoever held it before us likely already
        // refreshed, and the new token must not be refreshed again with the
        // now-consumed old refresh_token.
        let now = now_unix();
        let token = self
            .token
            .read()
            .await
            .clone()
            .ok_or(ProviderError::AuthRequired)?;
        if now < token.expires_at_unix.saturating_sub(60) {
            return Ok(token.access_token);
        }

        let refresh = token
            .refresh_token
            .clone()
            .ok_or(ProviderError::AuthRequired)?;
        self.refresh_with(refresh).await
    }

    /// Exchange `refresh` for a new token set and store it. Caller must hold
    /// `refresh_lock` — Spotify rotates PKCE refresh tokens, so this consumes
    /// the passed value.
    async fn refresh_with(&self, refresh: String) -> ProviderResult<String> {
        let client_id = self.client_id();
        let resp = self
            .http
            .post(SP_TOKEN)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", &client_id),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(%status, body = %body, "Spotify refresh failed");
            // Only a definitive rejection invalidates the stored credentials.
            // A transient 5xx/429/network blip must NOT delete the token file
            // and force the user back through the OAuth browser flow.
            if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
                self.disconnect().await?;
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Network(format!(
                "Spotify token refresh failed ({status})"
            )));
        }
        let tr: TokenResponseRaw = resp
            .json()
            .await
            .map_err(|e| ProviderError::Malformed(e.to_string()))?;
        let new_token = TokenSet {
            access_token: tr.access_token.clone(),
            refresh_token: tr.refresh_token.or(Some(refresh)),
            expires_at_unix: now_unix() + tr.expires_in.unwrap_or(3600),
        };
        self.commit_refreshed_token(new_token).await
    }

    async fn commit_refreshed_token(&self, new_token: TokenSet) -> ProviderResult<String> {
        let access_token = new_token.access_token.clone();
        let persisted = self.persist_token(&new_token);
        *self.token.write().await = Some(new_token);
        persisted?;
        Ok(access_token)
    }

    /// A 401 on an API call despite an unexpired access token means Spotify
    /// invalidated it early (credential rotation, suspend/resume, revocation).
    /// Force one refresh. Deduplicated against concurrent 401s via the same
    /// `refresh_lock`: whoever loses the race just adopts the winner's token
    /// instead of burning the already-rotated refresh token (which would 400
    /// and log the user out).
    async fn refresh_after_401(&self, stale_access: &str) -> ProviderResult<String> {
        let _refreshing = self.refresh_lock.lock().await;
        let token = self
            .token
            .read()
            .await
            .clone()
            .ok_or(ProviderError::AuthRequired)?;
        if token.access_token != stale_access {
            return Ok(token.access_token);
        }
        let refresh = token.refresh_token.ok_or(ProviderError::AuthRequired)?;
        self.refresh_with(refresh).await
    }

    async fn fetch_response(&self, url: &str) -> ProviderResult<Response> {
        let token = self.access_token().await?;
        let mut resp = self
            .http
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            // The token expired early or was invalidated server-side —
            // without this retry the app sat bricked for up to an hour
            // despite holding a perfectly good refresh token.
            let fresh = self.refresh_after_401(&token).await?;
            resp = self
                .http
                .get(url)
                .bearer_auth(&fresh)
                .send()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            if resp.status() == StatusCode::UNAUTHORIZED {
                return Err(ProviderError::AuthRequired);
            }
        }
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(5);
            return Err(ProviderError::RateLimited {
                retry_after_ms: retry * 1000,
            });
        }
        Ok(resp)
    }

    async fn fetch_json<T>(&self, url: &str) -> ProviderResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let resp = self.fetch_response(url).await?;
        if !resp.status().is_success() {
            return Err(ProviderError::Network(format!(
                "{url} -> {}",
                resp.status()
            )));
        }
        resp.json::<T>()
            .await
            .map_err(|e| ProviderError::Malformed(e.to_string()))
    }

    async fn fetch_json_allow_forbidden<T>(&self, url: &str) -> ProviderResult<Option<T>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let resp = self.fetch_response(url).await?;
        if resp.status() == StatusCode::FORBIDDEN {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(ProviderError::Network(format!(
                "{url} -> {}",
                resp.status()
            )));
        }
        resp.json::<T>()
            .await
            .map(Some)
            .map_err(|e| ProviderError::Malformed(e.to_string()))
    }
}

#[async_trait]
impl Provider for SpotifyProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Spotify
    }

    fn caps(&self) -> ProviderCaps {
        ProviderCaps {
            stream_feed: false,
            // Spotify deprecated audio-features/audio-analysis for new apps
            // in Nov 2024 — keep the bit off so discovery code knows not to
            // ask. Existing-app shims can flip this on per deployment.
            audio_features: false,
            playlists: true,
            reposts: false,
            // Playback is routed through the queue/player via librespot.
            playable: true,
        }
    }

    async fn search(&self, q: &Query) -> ProviderResult<SearchResults> {
        let url = spotify_search_url(&q.text, q.limit);
        let raw: SpSearchResp = self.fetch_json(&url).await?;
        Ok(SearchResults {
            tracks: raw.tracks.items.into_iter().map(sp_to_track).collect(),
            artists: raw
                .artists
                .map(|p| p.items.into_iter().map(sp_to_artist).collect())
                .unwrap_or_default(),
            playlists: raw
                .playlists
                .map(|page| spotify_search_playlists(page.items))
                .unwrap_or_default(),
        })
    }

    async fn track(&self, uri: &TrackUri) -> ProviderResult<Track> {
        let id = id_from_uri(&uri.0, "track")?;
        let url = format!("{SP_API}/tracks/{id}");
        let raw: SpTrack = self.fetch_json(&url).await?;
        Ok(sp_to_track(raw))
    }

    async fn artist(&self, uri: &ArtistUri) -> ProviderResult<Artist> {
        let id = id_from_uri(&uri.0, "artist")?;
        let url = format!("{SP_API}/artists/{id}");
        let raw: SpArtist = self.fetch_json(&url).await?;
        Ok(sp_to_artist(raw))
    }

    async fn resolve_stream(&self, _uri: &TrackUri) -> ProviderResult<StreamHandle> {
        // librespot integration lives in Phase 2B; until then Spotify tracks
        // can be browsed but not played in-process.
        Err(ProviderError::NotAvailable)
    }

    async fn artist_top_tracks(&self, uri: &ArtistUri, limit: u32) -> ProviderResult<Vec<Track>> {
        let id = id_from_uri(&uri.0, "artist")?;
        // `market=from_token` lets Spotify return the playable set for the
        // signed-in user's region. Without it Spotify rejects the request.
        let url = format!("{SP_API}/artists/{id}/top-tracks?market=from_token");
        let raw: SpTopTracks = self.fetch_json(&url).await?;
        Ok(raw
            .tracks
            .into_iter()
            .take(limit.clamp(1, 10) as usize)
            .map(sp_to_track)
            .collect())
    }

    async fn artist_albums(&self, uri: &ArtistUri, limit: u32) -> ProviderResult<Vec<AlbumBrief>> {
        let id = id_from_uri(&uri.0, "artist")?;
        // Own releases only — `appears_on` used to be included and flooded
        // the single 50-item page with various-artists compilations, pushing
        // the artist's actual discography out entirely.
        let limit = limit.clamp(1, 50);
        let url = format!(
            "{SP_API}/artists/{id}/albums?include_groups=album,single,compilation&limit={limit}&market=from_token"
        );
        let raw: SpAlbumsPage = self.fetch_json(&url).await?;
        Ok(raw.items.into_iter().map(sp_album_to_brief).collect())
    }

    async fn album(&self, uri: &AlbumUri) -> ProviderResult<AlbumDetail> {
        let id = id_from_uri(&uri.0, "album")?;
        let url = format!("{SP_API}/albums/{id}?market=from_token");
        let mut raw: SpAlbumFull = self.fetch_json(&url).await?;
        // The embedded tracks paging object caps at 50 — follow `next` so
        // long albums/compilations show their full tracklist. The page cap
        // is a runaway guard (~450 tracks total).
        let mut next = raw.tracks.next.take();
        let mut pages = 0;
        while let Some(page_url) = next {
            if pages >= 8 {
                tracing::warn!(album = %id, "album tracklist truncated at page cap");
                break;
            }
            pages += 1;
            let page: SpAlbumTracksPage = self.fetch_json(&page_url).await?;
            raw.tracks.items.extend(page.items);
            next = page.next;
        }
        Ok(sp_album_to_detail(raw))
    }

    async fn related_artists(
        &self,
        uri: &ArtistUri,
        limit: u32,
    ) -> ProviderResult<Vec<RelatedArtist>> {
        let id = id_from_uri(&uri.0, "artist")?;
        let url = format!("{SP_API}/artists/{id}/related-artists");
        let raw: SpRelatedArtists = self.fetch_json(&url).await?;
        Ok(raw
            .artists
            .into_iter()
            .take(limit.clamp(1, 20) as usize)
            .map(|a| RelatedArtist {
                uri: ArtistUri(format!("spotify:artist:{}", a.id)),
                provider: ProviderId::Spotify,
                name: a.name,
                image_url: sp_mid_image(a.images),
            })
            .collect())
    }
}

/// One page of liked-songs results plus the total count Spotify reports
/// (always present on `/me/tracks`). Callers use `total` to drive a progress
/// indicator while paginating.
#[derive(Debug, Clone)]
pub struct LikedTracksPage {
    pub tracks: Vec<Track>,
    pub total: u32,
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SpotifyPlaylistSummary {
    pub id: String,
    pub name: String,
    pub cover_url: Option<String>,
    pub track_count: usize,
}

#[derive(Debug, Clone)]
pub struct SpotifyPlaylistCatalog {
    pub playlists: Vec<SpotifyPlaylistSummary>,
    pub skipped_playlists: usize,
}

#[derive(Debug, Clone)]
pub struct SpotifyPlaylist {
    pub id: String,
    pub name: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct SpotifyPlaylistImport {
    pub playlists: Vec<SpotifyPlaylist>,
    pub skipped_playlists: usize,
    pub skipped_items: usize,
}

impl SpotifyProvider {
    /// Fetch a single page of liked songs starting at `offset`. The page size
    /// is capped at 50 by Spotify; we pass the value through directly.
    pub async fn liked_tracks_page(
        &self,
        offset: u32,
        limit: u32,
    ) -> ProviderResult<LikedTracksPage> {
        let limit = limit.clamp(1, 50);
        let url = format!("{SP_API}/me/tracks?limit={limit}&offset={offset}");
        let raw: SpLikedPage = self.fetch_json(&url).await?;
        // Offset math counts raw items (ghosts included) — Spotify's paging
        // does too, so skipping ghosts must not shift subsequent pages.
        let received = raw.items.len() as u32;
        let next_offset = if raw.next.is_some() && received == limit {
            Some(offset + received)
        } else {
            None
        };
        Ok(LikedTracksPage {
            tracks: liked_items_to_tracks(raw.items),
            total: raw.total,
            next_offset,
        })
    }

    /// Fetch *all* liked songs, calling `on_page` after each successful page.
    /// `on_page` receives the loaded slice and the running totals so the UI
    /// can append rows incrementally instead of waiting for the whole list.
    pub async fn liked_tracks_all<F>(&self, mut on_page: F) -> ProviderResult<()>
    where
        F: FnMut(Vec<Track>, u32, u32),
    {
        let mut offset: u32 = 0;
        loop {
            let page = self.liked_tracks_page(offset, 50).await?;
            let loaded_so_far = offset + page.tracks.len() as u32;
            let total = page.total;
            on_page(page.tracks, loaded_so_far, total);
            match page.next_offset {
                Some(next) => offset = next,
                None => return Ok(()),
            }
        }
    }

    /// List importable playlists without loading their track pages.
    pub async fn playlist_catalog_for_import(
        &self,
    ) -> ProviderResult<SpotifyPlaylistCatalog> {
        let me: SpProfile = self.fetch_json(&format!("{SP_API}/me")).await?;
        let mut offset = 0;
        let mut catalog = SpotifyPlaylistCatalog {
            playlists: Vec::new(),
            skipped_playlists: 0,
        };

        loop {
            let page: SpPlaylistsPage = self
                .fetch_json(&format!("{SP_API}/me/playlists?limit=50&offset={offset}"))
                .await?;
            let received = page.items.len();
            let page_catalog = playlist_summaries(&me.id, page.items);
            catalog.playlists.extend(page_catalog.playlists);
            catalog.skipped_playlists += page_catalog.skipped_playlists;
            match next_playlist_offset(offset, received, page.next.is_some()) {
                Some(next) => offset = next,
                None => return Ok(catalog),
            }
        }
    }

    /// Load only the playlists selected in the import dialog.
    pub async fn playlists_for_import(
        &self,
        selected: Vec<SpotifyPlaylistSummary>,
    ) -> ProviderResult<SpotifyPlaylistImport> {
        let mut imported = SpotifyPlaylistImport {
            playlists: Vec::new(),
            skipped_playlists: 0,
            skipped_items: 0,
        };
        for playlist in selected {
            match self.playlist_tracks(&playlist.id).await? {
                Some((tracks, skipped)) => {
                    imported.skipped_items += skipped;
                    imported.playlists.push(SpotifyPlaylist {
                        id: playlist.id,
                        name: playlist.name,
                        tracks,
                    });
                }
                None => imported.skipped_playlists += 1,
            }
        }
        Ok(imported)
    }

    async fn playlist_tracks(&self, id: &str) -> ProviderResult<Option<(Vec<Track>, usize)>> {
        let mut offset = 0;
        let mut tracks = Vec::new();
        let mut skipped = 0;

        loop {
            let Some(page): Option<SpPlaylistItemsPage> = self
                .fetch_json_allow_forbidden(&format!(
                    "{SP_API}/playlists/{id}/items?limit=50&offset={offset}"
                ))
                .await?
            else {
                return Ok(None);
            };
            let received = page.items.len();
            let (mut page_tracks, page_skipped) = playlist_items_to_tracks(page.items);
            tracks.append(&mut page_tracks);
            skipped += page_skipped;
            match next_playlist_offset(offset, received, page.next.is_some()) {
                Some(next) => offset = next,
                None => break,
            }
        }
        Ok(Some((tracks, skipped)))
    }
}

#[derive(Deserialize)]
struct SpLikedPage {
    items: Vec<SpLikedItem>,
    #[serde(default)]
    total: u32,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct SpLikedItem {
    /// `null` for ghost items — tracks pulled from the catalog after being
    /// saved. One such item used to fail the whole page's deserialisation
    /// and with it the entire liked-songs sync.
    #[serde(default)]
    track: Option<SpTrack>,
    /// ISO-8601 timestamp Spotify attaches to each saved track. We sort
    /// Home's "Recently liked" row on this. Optional only out of paranoia —
    /// the field is documented as always present.
    #[serde(default)]
    added_at: Option<DateTime<Utc>>,
}

/// Map a liked-songs page's items to tracks, silently dropping ghost items.
fn liked_items_to_tracks(items: Vec<SpLikedItem>) -> Vec<Track> {
    items
        .into_iter()
        .filter_map(|i| {
            let mut t = sp_to_track(i.track?);
            t.added_at = i.added_at;
            Some(t)
        })
        .collect()
}

fn playlist_items_to_tracks(items: Vec<SpPlaylistItem>) -> (Vec<Track>, usize) {
    let mut tracks = Vec::new();
    let mut skipped = 0;
    for item in items {
        let Some(value) = item.item else {
            skipped += 1;
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("track") {
            skipped += 1;
            continue;
        }
        match serde_json::from_value::<SpTrack>(value) {
            Ok(track) => tracks.push(sp_to_track(track)),
            Err(_) => skipped += 1,
        }
    }
    (tracks, skipped)
}

// ── PKCE & callback helpers ────────────────────────────────────────────────

fn spotify_search_url(text: &str, limit: Option<u32>) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("q", text)
        .append_pair("type", "track,artist,playlist")
        .append_pair(
            "limit",
            &limit.unwrap_or(SEARCH_MAX).clamp(1, SEARCH_MAX).to_string(),
        );
    format!("{SP_API}/search?{}", query.finish())
}

fn generate_pkce_pair() -> (String, String) {
    let verifier = random_unreserved(64);
    let hash = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);
    (verifier, challenge)
}

fn random_unreserved(len: usize) -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect()
}

fn random_alphanum(len: usize) -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect()
}

fn urlencoded(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn wait_for_callback(listener: TcpListener) -> ProviderResult<(String, String)> {
    // Time-bound the wait so a closed browser tab doesn't pin the listener.
    let accept = tokio::time::timeout(Duration::from_secs(300), listener.accept()).await;
    let (mut sock, _) = match accept {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(ProviderError::Other(format!("callback accept: {e}"))),
        Err(_) => {
            return Err(ProviderError::Other(
                "OAuth callback timed out (5 min) — browser tab never came back".into(),
            ));
        }
    };

    let mut buf = vec![0u8; 8192];
    let n = sock
        .read(&mut buf)
        .await
        .map_err(|e| ProviderError::Other(format!("callback read: {e}")))?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("");
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");

    let mut code = String::new();
    let mut state = String::new();
    let mut error: Option<String> = None;
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" => code = v.into_owned(),
            "state" => state = v.into_owned(),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    let html_ok = "<html><head><title>nira</title><style>body{font-family:system-ui;background:#0b0b0d;color:#e8e8ea;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}h1{color:#c9a35d;margin:0 0 12px}p{color:#8a8a92}</style></head><body><div style=\"text-align:center\"><h1>Connected.</h1><p>You can close this tab.</p></div></body></html>";
    let html_err = "<html><head><title>nira</title><style>body{font-family:system-ui;background:#0b0b0d;color:#e8e8ea;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}h1{color:#e07474;margin:0 0 12px}p{color:#8a8a92}</style></head><body><div style=\"text-align:center\"><h1>Sign-in failed.</h1><p>You can close this tab and try again in nira.</p></div></body></html>";
    let html = if error.is_some() { html_err } else { html_ok };

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        html.len()
    );
    let _ = sock.write_all(header.as_bytes()).await;
    let _ = sock.write_all(html.as_bytes()).await;
    let _ = sock.shutdown().await;

    if let Some(err) = error {
        return Err(ProviderError::Other(format!("OAuth error: {err}")));
    }
    if code.is_empty() {
        return Err(ProviderError::Other(
            "OAuth callback had no `code` parameter".into(),
        ));
    }
    Ok((code, state))
}

fn id_from_uri(uri: &str, kind: &str) -> ProviderResult<String> {
    let parts: Vec<&str> = uri.split(':').collect();
    if parts.len() != 3 || parts[0] != "spotify" || parts[1] != kind {
        return Err(ProviderError::Malformed(format!(
            "expected spotify:{kind}:<id>, got {uri}"
        )));
    }
    Ok(parts[2].to_string())
}

// ── Spotify Web API shapes (minimal) ───────────────────────────────────────

#[derive(Deserialize)]
struct SpSearchResp {
    tracks: SpTrackPage,
    #[serde(default)]
    artists: Option<SpArtistPage>,
    #[serde(default)]
    playlists: Option<SpPlaylistSearchPage>,
}

#[derive(Deserialize)]
struct SpPlaylistSearchPage {
    #[serde(default)]
    items: Vec<Option<SpPlaylistBrief>>,
}

#[derive(Deserialize)]
struct SpArtistPage {
    items: Vec<SpArtist>,
}

#[derive(Deserialize)]
struct SpTrackPage {
    items: Vec<SpTrack>,
}

#[derive(Deserialize)]
struct SpProfile {
    id: String,
}

#[derive(Deserialize)]
struct SpPlaylistsPage {
    #[serde(default)]
    items: Vec<SpPlaylistBrief>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct SpPlaylistBrief {
    id: String,
    name: String,
    #[serde(default)]
    collaborative: bool,
    owner: SpPlaylistOwner,
    images: Option<Vec<SpImage>>,
    #[serde(default)]
    external_urls: SpExternalUrls,
    #[serde(default)]
    items: Option<SpPlaylistItemsRef>,
    #[serde(default)]
    tracks: Option<SpPlaylistItemsRef>,
}

#[derive(Deserialize)]
struct SpPlaylistOwner {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct SpPlaylistItemsRef {
    #[serde(default)]
    total: usize,
}

fn spotify_search_playlists(items: Vec<Option<SpPlaylistBrief>>) -> Vec<PlaylistBrief> {
    items
        .into_iter()
        .flatten()
        .filter_map(|playlist| {
            let external_url = playlist.external_urls.spotify?;
            let kind = if playlist.owner.id == "spotify" {
                PlaylistKind::Editorial
            } else {
                PlaylistKind::User
            };
            Some(PlaylistBrief {
                uri: PlaylistUri(format!("spotify:playlist:{}", playlist.id)),
                provider: ProviderId::Spotify,
                title: playlist.name,
                owner_name: playlist.owner.display_name.or(Some(playlist.owner.id)),
                cover_url: sp_mid_image(playlist.images.unwrap_or_default()),
                track_count: playlist.items.or(playlist.tracks).map(|items| items.total),
                kind,
                open: PlaylistOpen::External(external_url),
            })
        })
        .collect()
}

fn playlist_summaries(
    current_user_id: &str,
    items: Vec<SpPlaylistBrief>,
) -> SpotifyPlaylistCatalog {
    let mut catalog = SpotifyPlaylistCatalog {
        playlists: Vec::new(),
        skipped_playlists: 0,
    };
    for playlist in items {
        if playlist.owner.id != current_user_id && !playlist.collaborative {
            catalog.skipped_playlists += 1;
            continue;
        }
        catalog.playlists.push(SpotifyPlaylistSummary {
            id: playlist.id,
            name: playlist.name,
            cover_url: playlist
                .images
                .unwrap_or_default()
                .into_iter()
                .next()
                .map(|image| image.url),
            track_count: playlist
                .items
                .or(playlist.tracks)
                .map(|items| items.total)
                .unwrap_or_default(),
        });
    }
    catalog
}

fn next_playlist_offset(
    offset: usize,
    received: usize,
    has_next: bool,
) -> Option<usize> {
    (has_next && received > 0).then_some(offset + received)
}

#[derive(Deserialize)]
struct SpPlaylistItemsPage {
    #[serde(default)]
    items: Vec<SpPlaylistItem>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct SpPlaylistItem {
    #[serde(default)]
    item: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct SpTrack {
    id: String,
    name: String,
    duration_ms: u64,
    artists: Vec<SpArtistRef>,
    album: SpAlbumRef,
}

#[derive(Deserialize)]
struct SpArtistRef {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct SpAlbumRef {
    id: String,
    name: String,
    #[serde(default)]
    images: Vec<SpImage>,
}

#[derive(Deserialize)]
struct SpArtist {
    id: String,
    name: String,
    #[serde(default)]
    images: Vec<SpImage>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    external_urls: SpExternalUrls,
}

#[derive(Default, Deserialize)]
struct SpExternalUrls {
    #[serde(default)]
    spotify: Option<String>,
}

#[derive(Deserialize)]
struct SpImage {
    url: String,
}

fn sp_to_track(sp: SpTrack) -> Track {
    Track {
        uri: TrackUri(format!("spotify:track:{}", sp.id)),
        provider: ProviderId::Spotify,
        title: sp.name,
        artists: sp
            .artists
            .into_iter()
            .map(|a| ArtistRef {
                uri: ArtistUri(format!("spotify:artist:{}", a.id)),
                name: a.name,
            })
            .collect(),
        album: Some(AlbumRef {
            uri: AlbumUri(format!("spotify:album:{}", sp.album.id)),
            title: sp.album.name,
        }),
        duration: Duration::from_millis(sp.duration_ms),
        cover_url: sp_mid_image(sp.album.images),
        mbid: None,
        added_at: None,
    }
}

/// Spotify orders `images` largest-first (typically 640/300/64). Cards and
/// rows render covers at ~170 px, so the middle size decodes ~4× cheaper
/// than the 640 px original with no visible loss. Detail heroes keep the
/// full-size first entry.
fn sp_mid_image(mut images: Vec<SpImage>) -> Option<String> {
    if images.is_empty() {
        return None;
    }
    let mid = images.len() / 2;
    Some(images.swap_remove(mid).url)
}

fn sp_to_artist(raw: SpArtist) -> Artist {
    Artist {
        uri: ArtistUri(format!("spotify:artist:{}", raw.id)),
        provider: ProviderId::Spotify,
        name: raw.name,
        image_url: sp_mid_image(raw.images),
        genres: raw.genres,
        permalink_url: raw.external_urls.spotify,
    }
}

fn sp_album_type(s: &str) -> AlbumType {
    match s.to_lowercase().as_str() {
        "album" => AlbumType::Album,
        "single" => AlbumType::Single,
        "ep" => AlbumType::Ep,
        "compilation" => AlbumType::Compilation,
        _ => AlbumType::Unknown,
    }
}

fn year_from_release(date: &str) -> Option<u32> {
    // Spotify gives one of YYYY / YYYY-MM / YYYY-MM-DD depending on
    // `release_date_precision`. Slicing the first four chars covers all
    // three; parse failures simply drop to None.
    date.get(..4).and_then(|s| s.parse().ok())
}

fn sp_album_to_brief(a: SpAlbumBrief) -> AlbumBrief {
    AlbumBrief {
        uri: AlbumUri(format!("spotify:album:{}", a.id)),
        provider: ProviderId::Spotify,
        title: a.name,
        artist_name: a
            .artists
            .into_iter()
            .map(|x| x.name)
            .collect::<Vec<_>>()
            .join(", "),
        cover_url: sp_mid_image(a.images),
        release_year: a.release_date.as_deref().and_then(year_from_release),
        total_tracks: a.total_tracks,
        album_type: a
            .album_type
            .as_deref()
            .map(sp_album_type)
            .unwrap_or(AlbumType::Unknown),
    }
}

fn sp_album_to_detail(a: SpAlbumFull) -> AlbumDetail {
    let cover_url = a.images.first().map(|i| i.url.clone());
    let release_year = a.release_date.as_deref().and_then(year_from_release);
    let album_type = a
        .album_type
        .as_deref()
        .map(sp_album_type)
        .unwrap_or(AlbumType::Unknown);
    let artist_ref = a
        .artists
        .first()
        .map(|x| ArtistRef {
            uri: ArtistUri(format!("spotify:artist:{}", x.id)),
            name: x.name.clone(),
        })
        .unwrap_or_else(|| ArtistRef {
            uri: ArtistUri(String::new()),
            name: String::new(),
        });
    // Each track in /v1/albums/{id}/tracks lacks the parent album payload;
    // we paste it back on so the UI has a cover/album label per row.
    let album_label = AlbumRef {
        uri: AlbumUri(format!("spotify:album:{}", a.id)),
        title: a.name.clone(),
    };
    let tracks: Vec<Track> = a
        .tracks
        .items
        .into_iter()
        .map(|t| Track {
            uri: TrackUri(format!("spotify:track:{}", t.id)),
            provider: ProviderId::Spotify,
            title: t.name,
            artists: t
                .artists
                .into_iter()
                .map(|x| ArtistRef {
                    uri: ArtistUri(format!("spotify:artist:{}", x.id)),
                    name: x.name,
                })
                .collect(),
            album: Some(album_label.clone()),
            duration: Duration::from_millis(t.duration_ms),
            cover_url: cover_url.clone(),
            mbid: None,
            added_at: None,
        })
        .collect();
    AlbumDetail {
        uri: AlbumUri(format!("spotify:album:{}", a.id)),
        provider: ProviderId::Spotify,
        title: a.name,
        artist: artist_ref,
        cover_url,
        release_year,
        album_type,
        tracks,
    }
}

// ── New DTOs for artist / album endpoints ───────────────────────────────────

#[derive(Deserialize)]
struct SpTopTracks {
    #[serde(default)]
    tracks: Vec<SpTrack>,
}

#[derive(Deserialize)]
struct SpAlbumsPage {
    #[serde(default)]
    items: Vec<SpAlbumBrief>,
}

#[derive(Deserialize)]
struct SpAlbumBrief {
    id: String,
    name: String,
    #[serde(default)]
    images: Vec<SpImage>,
    #[serde(default)]
    artists: Vec<SpArtistRef>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    total_tracks: Option<u32>,
    #[serde(default)]
    album_type: Option<String>,
}

#[derive(Deserialize)]
struct SpAlbumFull {
    id: String,
    name: String,
    #[serde(default)]
    images: Vec<SpImage>,
    #[serde(default)]
    artists: Vec<SpArtistRef>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    album_type: Option<String>,
    tracks: SpAlbumTracksPage,
}

#[derive(Deserialize)]
struct SpAlbumTracksPage {
    #[serde(default)]
    items: Vec<SpAlbumTrack>,
    /// Absolute URL of the next 50-track page, when the album has more.
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct SpAlbumTrack {
    id: String,
    name: String,
    #[serde(default)]
    artists: Vec<SpArtistRef>,
    #[serde(default)]
    duration_ms: u64,
}

#[derive(Deserialize)]
struct SpRelatedArtists {
    #[serde(default)]
    artists: Vec<SpArtist>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nira-spotify-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[tokio::test]
    async fn disconnect_keeps_memory_when_token_file_cannot_be_removed() {
        let dir = token_test_dir("disconnect-error");
        let token_path = dir.join("spotify-tokens.json");
        std::fs::create_dir_all(&token_path).unwrap();
        let provider = SpotifyProvider::new("client".into(), Some(token_path)).unwrap();
        *provider.token.write().await = Some(TokenSet {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            expires_at_unix: 123,
        });

        let error = provider.disconnect().await.unwrap_err();

        assert!(!error.to_string().is_empty());
        assert!(provider.is_connected());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refreshed_token_updates_memory_but_returns_the_disk_error() {
        let dir = token_test_dir("refresh-write-error");
        std::fs::create_dir_all(&dir).unwrap();
        let parent_is_file = dir.join("not-a-directory");
        std::fs::write(&parent_is_file, b"keep").unwrap();
        let provider = SpotifyProvider::new(
            "client".into(),
            Some(parent_is_file.join("spotify-tokens.json")),
        )
        .unwrap();

        let error = provider
            .commit_refreshed_token(TokenSet {
                access_token: "new-access".into(),
                refresh_token: Some("new-refresh".into()),
                expires_at_unix: 456,
            })
            .await
            .unwrap_err();

        assert!(!error.to_string().is_empty());
        let memory = provider.token.read().await.clone().unwrap();
        assert_eq!(memory.access_token, "new-access");
        assert_eq!(memory.refresh_token.as_deref(), Some("new-refresh"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn token_persist_replaces_symlink_with_private_file() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = token_test_dir("symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let victim_path = dir.join("victim");
        let token_path = dir.join("spotify-tokens.json");
        std::fs::write(&victim_path, "untouched").unwrap();
        symlink(&victim_path, &token_path).unwrap();

        let provider = SpotifyProvider::new("client".into(), Some(token_path.clone())).unwrap();
        provider
            .persist_token(&TokenSet {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                expires_at_unix: 123,
            })
            .unwrap();

        let victim = std::fs::read_to_string(&victim_path).unwrap();
        let saved: TokenSet =
            serde_json::from_str(&std::fs::read_to_string(&token_path).unwrap()).unwrap();
        let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
        std::fs::remove_dir_all(dir).unwrap();

        assert_eq!(victim, "untouched");
        assert_eq!(saved.access_token, "access");
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn ghost_liked_item_is_skipped_not_fatal() {
        // Real-world shape: one saved track was pulled from the catalog and
        // comes back as `track: null` — the page must still parse and the
        // ghost must vanish from the mapped tracks.
        let json = r#"{
            "items": [
                { "track": null, "added_at": "2026-07-01T00:00:00Z" },
                { "track": {
                    "id": "t1", "name": "Alive", "duration_ms": 1000,
                    "artists": [{ "id": "a1", "name": "A" }],
                    "album": { "id": "al1", "name": "Al", "images": [] }
                  }, "added_at": "2026-07-02T00:00:00Z" }
            ],
            "total": 2
        }"#;
        let page: SpLikedPage = serde_json::from_str(json).expect("page with ghost parses");
        assert_eq!(page.items.len(), 2);
        let tracks = liked_items_to_tracks(page.items);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Alive");
        assert!(tracks[0].added_at.is_some());
    }

    #[test]
    fn playlist_items_keep_tracks_and_drop_unsupported_items() {
        let json = r#"{
            "items": [
                { "item": {
                    "type": "track",
                    "id": "t1", "name": "Alive", "duration_ms": 1000,
                    "artists": [{ "id": "a1", "name": "A" }],
                    "album": { "id": "al1", "name": "Al", "images": [] }
                }},
                { "item": {
                    "type": "episode",
                    "id": "ep1", "name": "A podcast"
                }}
            ],
            "next": null
        }"#;
        let page: SpPlaylistItemsPage = serde_json::from_str(json).expect("playlist page parses");
        let (tracks, skipped) = playlist_items_to_tracks(page.items);

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Alive");
        assert_eq!(skipped, 1);
    }

    #[test]
    fn playlist_catalog_keeps_owned_and_collaborative_summaries() {
        let json = r#"{
            "items": [
                {
                    "id": "owned",
                    "name": "Owned",
                    "collaborative": false,
                    "owner": { "id": "me" },
                    "images": [{ "url": "https://i.scdn.co/owned.jpg" }],
                    "items": { "total": 12 },
                    "tracks": { "total": 12 }
                },
                {
                    "id": "collab",
                    "name": "Collab",
                    "collaborative": true,
                    "owner": { "id": "friend" },
                    "images": null,
                    "tracks": { "total": 4 }
                },
                {
                    "id": "followed",
                    "name": "Followed",
                    "collaborative": false,
                    "owner": { "id": "friend" },
                    "images": [],
                    "items": { "total": 9 }
                }
            ],
            "next": null
        }"#;
        let page: SpPlaylistsPage =
            serde_json::from_str(json).expect("playlist page parses");
        let catalog = playlist_summaries("me", page.items);

        assert_eq!(catalog.playlists.len(), 2);
        assert_eq!(catalog.skipped_playlists, 1);
        assert_eq!(catalog.playlists[0].track_count, 12);
        assert_eq!(
            catalog.playlists[0].cover_url.as_deref(),
            Some("https://i.scdn.co/owned.jpg")
        );
        assert_eq!(catalog.playlists[1].track_count, 4);
    }

    #[test]
    fn playlist_pagination_advances_by_received_items_only() {
        assert_eq!(next_playlist_offset(0, 50, true), Some(50));
        assert_eq!(next_playlist_offset(50, 17, true), Some(67));
        assert_eq!(next_playlist_offset(50, 0, true), None);
        assert_eq!(next_playlist_offset(50, 17, false), None);
    }

    #[test]
    fn search_url_caps_limit_for_current_development_mode() {
        let url = spotify_search_url("massive attack", Some(15));

        assert_eq!(
            url,
            "https://api.spotify.com/v1/search?q=massive+attack&type=track%2Cartist%2Cplaylist&limit=10"
        );
    }

    #[test]
    fn search_playlists_keep_kind_owner_count_and_external_url() {
        let json = r#"{
            "items": [
                {
                    "id": "editorial",
                    "name": "Fresh Finds",
                    "owner": { "id": "spotify", "display_name": "Spotify" },
                    "external_urls": { "spotify": "https://open.spotify.com/playlist/editorial" },
                    "images": [],
                    "items": { "total": 25 }
                },
                {
                    "id": "community",
                    "name": "Night drive",
                    "owner": { "id": "listener", "display_name": "Mira" },
                    "external_urls": { "spotify": "https://open.spotify.com/playlist/community" },
                    "images": [],
                    "tracks": { "total": 12 }
                }
            ]
        }"#;
        let page: SpPlaylistSearchPage = serde_json::from_str(json).unwrap();
        let playlists = spotify_search_playlists(page.items);

        assert_eq!(playlists[0].kind, provider_api::PlaylistKind::Editorial);
        assert_eq!(playlists[1].kind, provider_api::PlaylistKind::User);
        assert_eq!(playlists[1].owner_name.as_deref(), Some("Mira"));
        assert_eq!(playlists[1].track_count, Some(12));
        assert_eq!(
            playlists[1].open,
            provider_api::PlaylistOpen::External(
                "https://open.spotify.com/playlist/community".into()
            )
        );
    }
}
