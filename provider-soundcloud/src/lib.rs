//! SoundCloud provider.
//!
//! SoundCloud no longer issues new public API credentials, so every
//! community-built client extracts a `client_id` from the web player's JS
//! bundles. We do the same — it's the established gray-area approach. The
//! token is refreshed when a request comes back 401.
//!
//! Endpoints used (all hit `api-v2.soundcloud.com`, the same backend the
//! web player talks to):
//!   - `/search/tracks?q=&limit=&client_id=`
//!   - `/tracks/{id}?client_id=`
//!   - transcoding `url?client_id=` → JSON `{ "url": "cf-media…" }`
//!
//! We deliberately keep the deserialisable structs minimal — SoundCloud
//! returns hundreds of fields per track, but every additional field is one
//! more chance to break when their backend tweaks something.

use std::time::Duration;

use async_trait::async_trait;
use provider_api::{
    Artist, ArtistRef, ArtistUri, Provider, ProviderCaps, ProviderError, ProviderId,
    ProviderResult, Query, SearchResults, StreamHandle, Track, TrackUri,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const SC_HOME: &str = "https://soundcloud.com/";
const SC_API: &str = "https://api-v2.soundcloud.com";

#[derive(Serialize, Deserialize)]
struct ClientIdCache {
    client_id: String,
}

pub struct SoundCloudProvider {
    http: Client,
    client_id: RwLock<Option<String>>,
}

impl SoundCloudProvider {
    pub fn new() -> ProviderResult<Self> {
        let http = Client::builder()
            .user_agent(UA)
            .build()
            .map_err(|e| ProviderError::Other(format!("http client: {e}")))?;
        // Pick up the last-known client_id from disk so the first search
        // doesn't pay the 1-2s web-scrape round trip on every launch.
        let cached = config::AppConfig::soundcloud_client_id_cache_path()
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .and_then(|s| serde_json::from_str::<ClientIdCache>(&s).ok())
            .map(|c| c.client_id);
        Ok(Self {
            http,
            client_id: RwLock::new(cached),
        })
    }

    /// Fire-and-forget resolve so the first user-visible search doesn't pay
    /// the auto-detect cost. Safe to call at app boot.
    pub async fn prewarm(&self) {
        let _ = self.client_id().await;
    }

    pub fn has_cached_client_id(&self) -> bool {
        self.client_id
            .try_read()
            .map(|id| id.is_some())
            .unwrap_or(false)
    }

    pub async fn clear_client_id_cache(&self) -> ProviderResult<()> {
        *self.client_id.write().await = None;
        if let Some(path) = config::AppConfig::soundcloud_client_id_cache_path()
            && let Err(e) = std::fs::remove_file(path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(ProviderError::Other(format!("client_id cache clear: {e}")));
        }
        Ok(())
    }

    /// Cached client_id getter. Hot path is a single RwLock read; cold path
    /// drops to the web-scrape resolver below.
    async fn client_id(&self) -> ProviderResult<String> {
        {
            let r = self.client_id.read().await;
            if let Some(id) = r.as_ref() {
                return Ok(id.clone());
            }
        }
        let mut w = self.client_id.write().await;
        if let Some(id) = w.as_ref() {
            return Ok(id.clone());
        }
        let id = self.resolve_client_id().await?;
        tracing::info!("soundcloud client_id resolved");
        *w = Some(id.clone());
        // Persist for the next launch. Failures here are non-fatal — worst
        // case we just re-resolve on the next start.
        if let Some(path) = config::AppConfig::soundcloud_client_id_cache_path()
            && let Err(e) = config::AppConfig::atomic_write_json(
                &path,
                &ClientIdCache {
                    client_id: id.clone(),
                },
            )
        {
            tracing::warn!(error = %e, "could not persist SC client_id");
        }
        Ok(id)
    }

    /// Invalidate the cached client_id (e.g. after a 401) and resolve again.
    pub async fn refresh_client_id(&self) -> ProviderResult<String> {
        let mut w = self.client_id.write().await;
        *w = None;
        drop(w);
        self.client_id().await
    }

    async fn resolve_client_id(&self) -> ProviderResult<String> {
        let html = self.fetch_text(SC_HOME).await?;
        let mut scripts: Vec<String> = Vec::new();
        for chunk in html.split("<script") {
            // Crude but stable: each <script> tag chunk has `src="…"`.
            if let Some(i) = chunk.find("src=\"") {
                let rest = &chunk[i + 5..];
                if let Some(j) = rest.find('"') {
                    let url = &rest[..j];
                    if url.contains("sndcdn.com/assets/") && url.ends_with(".js") {
                        scripts.push(url.to_string());
                    }
                }
            }
        }
        // Try from the last bundle backwards — client_id is typically in a
        // late chunk. Bail as soon as one yields a plausible token.
        for url in scripts.iter().rev() {
            if let Ok(body) = self.fetch_text(url).await
                && let Some(id) = extract_client_id(&body)
            {
                return Ok(id);
            }
        }
        Err(ProviderError::Other(
            "could not auto-detect SoundCloud client_id from web player".into(),
        ))
    }

    async fn fetch_text(&self, url: &str) -> ProviderResult<String> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ProviderError::Network(format!(
                "GET {url} -> {}",
                resp.status()
            )));
        }
        resp.text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))
    }

    async fn fetch_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> ProviderResult<T> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }
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

    /// Wrap a call that takes a client_id, retrying once after refresh on
    /// auth failure. SoundCloud rotates its web client_ids occasionally; the
    /// next call after a 401 picks up the new one transparently.
    async fn with_client_id<F, Fut, T>(&self, f: F) -> ProviderResult<T>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = ProviderResult<T>>,
    {
        let id = self.client_id().await?;
        match f(id).await {
            Err(ProviderError::AuthRequired) => {
                let fresh = self.refresh_client_id().await?;
                f(fresh).await
            }
            other => other,
        }
    }
}

fn extract_client_id(js: &str) -> Option<String> {
    // SC bundles render `,client_id:"<32-alphanum>"` somewhere in the
    // minified output. The leading comma rules out other id-looking strings
    // that aren't the auth token.
    let needle = ",client_id:\"";
    let i = js.find(needle)? + needle.len();
    let rest = &js[i..];
    let j = rest.find('"')?;
    let candidate = &rest[..j];
    if candidate.len() >= 16 && candidate.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn track_id_from_uri(uri: &TrackUri) -> ProviderResult<u64> {
    let parts: Vec<&str> = uri.0.split(':').collect();
    if parts.len() != 3 || parts[0] != "soundcloud" || parts[1] != "track" {
        return Err(ProviderError::Malformed(format!(
            "expected soundcloud:track:<id>, got {}",
            uri.0
        )));
    }
    parts[2]
        .parse::<u64>()
        .map_err(|e| ProviderError::Malformed(format!("track id parse: {e}")))
}

fn user_id_from_uri(uri: &ArtistUri) -> ProviderResult<u64> {
    let parts: Vec<&str> = uri.0.split(':').collect();
    if parts.len() != 3 || parts[0] != "soundcloud" || parts[1] != "user" {
        return Err(ProviderError::Malformed(format!(
            "expected soundcloud:user:<id>, got {}",
            uri.0
        )));
    }
    parts[2]
        .parse::<u64>()
        .map_err(|e| ProviderError::Malformed(format!("user id parse: {e}")))
}

#[derive(Deserialize)]
struct ScUserFull {
    id: u64,
    username: String,
    #[serde(default)]
    avatar_url: Option<String>,
    #[serde(default)]
    permalink_url: Option<String>,
}

#[async_trait]
impl Provider for SoundCloudProvider {
    fn id(&self) -> ProviderId {
        ProviderId::SoundCloud
    }

    fn caps(&self) -> ProviderCaps {
        ProviderCaps {
            stream_feed: true,
            audio_features: false,
            playlists: true,
            reposts: true,
            playable: true,
        }
    }

    async fn search(&self, q: &Query) -> ProviderResult<SearchResults> {
        let limit = q.limit.unwrap_or(20).clamp(1, 50);
        let encoded = url::form_urlencoded::byte_serialize(q.text.as_bytes()).collect::<String>();
        self.with_client_id(|cid| {
            let url = format!("{SC_API}/search/tracks?q={encoded}&limit={limit}&client_id={cid}");
            async move {
                let raw: ScSearchResp = self.fetch_json(&url).await?;
                let tracks = raw.collection.into_iter().map(sc_to_track).collect();
                Ok(SearchResults {
                    tracks,
                    artists: Vec::new(),
                })
            }
        })
        .await
    }

    async fn track(&self, uri: &TrackUri) -> ProviderResult<Track> {
        let id = track_id_from_uri(uri)?;
        self.with_client_id(|cid| {
            let url = format!("{SC_API}/tracks/{id}?client_id={cid}");
            async move {
                let raw: ScTrack = self.fetch_json(&url).await?;
                Ok(sc_to_track(raw))
            }
        })
        .await
    }

    async fn artist(&self, uri: &ArtistUri) -> ProviderResult<Artist> {
        let id = user_id_from_uri(uri)?;
        self.with_client_id(|cid| {
            let url = format!("{SC_API}/users/{id}?client_id={cid}");
            async move {
                let raw: ScUserFull = self.fetch_json(&url).await?;
                Ok(Artist {
                    uri: ArtistUri(format!("soundcloud:user:{}", raw.id)),
                    provider: ProviderId::SoundCloud,
                    name: raw.username,
                    image_url: raw.avatar_url.map(upgrade_artwork),
                    // SC has no concept of "genres" per artist — leave empty
                    // so the UI's genre-chip row collapses cleanly.
                    genres: Vec::new(),
                    permalink_url: raw.permalink_url,
                })
            }
        })
        .await
    }

    async fn artist_top_tracks(&self, uri: &ArtistUri, limit: u32) -> ProviderResult<Vec<Track>> {
        self.user_tracks(uri, limit).await
    }
    // Albums + album detail + related artists deliberately fall through to
    // the trait's default-not-available impls; SC has no first-class album
    // concept and no related-artists endpoint we can rely on long-term.

    async fn resolve_stream(&self, uri: &TrackUri) -> ProviderResult<StreamHandle> {
        let id = track_id_from_uri(uri)?;
        self.with_client_id(|cid| {
            let track_url = format!("{SC_API}/tracks/{id}?client_id={cid}");
            async move {
                let track: ScTrack = self.fetch_json(&track_url).await?;
                let transcoding = pick_stream(&track.media.transcodings)
                    .ok_or(ProviderError::NotAvailable)?
                    .clone();
                let separator = if transcoding.url.contains('?') {
                    "&"
                } else {
                    "?"
                };
                let resolved: ScStreamResp = self
                    .fetch_json(&format!("{}{separator}client_id={cid}", transcoding.url))
                    .await?;
                match transcoding.format.protocol.as_str() {
                    // Progressive: a single CDN URL the caller downloads.
                    "progressive" => Ok(StreamHandle::HttpStream {
                        url: resolved.url,
                        content_type: Some(transcoding.format.mime_type.clone()),
                    }),
                    // HLS: resolved.url is an m3u8 playlist; we fetch the
                    // segments and hand back one concatenated buffer the
                    // audio engine can decode in-place. Symphonia re-syncs
                    // at MPEG-TS / ID3 frame boundaries, so byte-concat is
                    // safe for the typical audio-only stream SC serves.
                    "hls" => {
                        let bytes = self.fetch_hls_audio(&resolved.url).await?;
                        Ok(StreamHandle::Bytes {
                            data: bytes,
                            content_type: Some(transcoding.format.mime_type.clone()),
                        })
                    }
                    other => Err(ProviderError::Other(format!(
                        "unsupported SC transcoding protocol: {other}"
                    ))),
                }
            }
        })
        .await
    }
}

impl SoundCloudProvider {
    /// Download an m3u8 playlist + all referenced segments and return one
    /// elementary-stream buffer. SC's `audio/mpeg` HLS variant wraps MP3
    /// frames inside 188-byte MPEG-TS packets, so a raw byte-concat gives
    /// symphonia garbled audio. `demux_ts_audio_es` peels the TS off and
    /// hands back the ES (MP3 ADTS frames) symphonia can decode directly.
    /// Segments are fetched sequentially because SC's CDN throttles per-IP
    /// burst — parallel-fetching often costs more in 429-induced retry than
    /// it saves in wall time.
    async fn fetch_hls_audio(&self, playlist_url: &str) -> ProviderResult<Vec<u8>> {
        let m3u8 = self.fetch_text(playlist_url).await?;
        let segments = parse_m3u8_segments(&m3u8, playlist_url);
        if segments.is_empty() {
            return Err(ProviderError::Malformed(
                "m3u8 contained no audio segments".into(),
            ));
        }
        let total = segments.len();
        let mut buf: Vec<u8> = Vec::with_capacity(1024 * 1024);
        let mut ok = 0usize;
        for url in &segments {
            match self.http.get(url).send().await {
                Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                    Ok(b) => {
                        // Peel MPEG-TS wrapper, fall back to raw bytes if
                        // the segment doesn't look like TS (some SC
                        // variants ship raw ADTS/ES already).
                        let es = demux_ts_audio_es(&b);
                        if es.is_empty() {
                            buf.extend_from_slice(&b);
                        } else {
                            buf.extend_from_slice(&es);
                        }
                        ok += 1;
                    }
                    Err(e) => tracing::warn!(error = %e, "SC HLS segment body"),
                },
                Ok(resp) => tracing::warn!(status = %resp.status(), "SC HLS segment status"),
                Err(e) => tracing::warn!(error = %e, "SC HLS segment fetch"),
            }
        }
        if buf.is_empty() {
            return Err(ProviderError::Network(
                "SC HLS produced empty buffer".into(),
            ));
        }
        tracing::debug!(ok, total, bytes = buf.len(), "SC HLS download complete");
        Ok(buf)
    }
}

/// Strip an MPEG-TS wrapper down to its audio elementary stream.
///
/// SC's `audio/mpeg` HLS segments are 188-byte TS packets carrying MP3
/// inside PES packets. We walk the packets, pick the first PID whose PUSI
/// payload starts with an MPEG-audio stream_id (0xC0..0xDF), and
/// concatenate the PES payload (skipping the variable-length PES header)
/// for every subsequent packet on that PID. The result is a continuous
/// MP3 byte stream symphonia decodes directly.
///
/// Returns an empty Vec if the input isn't recognisable TS (no 0x47
/// sync) or has no audio PES — callers fall back to raw bytes.
fn demux_ts_audio_es(buf: &[u8]) -> Vec<u8> {
    // Find first 0x47 sync — some streams have an offset (rare for SC,
    // but cheap to scan).
    let start = buf.iter().position(|&b| b == 0x47).unwrap_or(buf.len());
    let body = &buf[start..];
    if body.len() < 188 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(body.len());
    let mut audio_pid: Option<u16> = None;
    for pkt in body.chunks_exact(188) {
        if pkt[0] != 0x47 {
            continue;
        }
        let pusi = (pkt[1] & 0x40) != 0;
        let pid = (((pkt[1] & 0x1F) as u16) << 8) | (pkt[2] as u16);
        let afc = (pkt[3] >> 4) & 0x03;
        let has_payload = afc & 0x01 != 0;
        let has_af = afc & 0x02 != 0;
        if !has_payload {
            continue;
        }
        let mut payload_start = 4usize;
        if has_af {
            let af_len = pkt[4] as usize;
            payload_start = 5 + af_len;
        }
        if payload_start >= 188 {
            continue;
        }
        let payload = &pkt[payload_start..];

        if pusi
            && payload.len() >= 9
            && payload[0] == 0x00
            && payload[1] == 0x00
            && payload[2] == 0x01
        {
            let stream_id = payload[3];
            let is_audio = (0xC0..=0xDF).contains(&stream_id);
            if !is_audio {
                continue;
            }
            if audio_pid.is_none() {
                audio_pid = Some(pid);
            }
            if audio_pid != Some(pid) {
                continue;
            }
            let pes_header_data_len = payload[8] as usize;
            let es_start = 9 + pes_header_data_len;
            if es_start < payload.len() {
                out.extend_from_slice(&payload[es_start..]);
            }
            continue;
        }

        if audio_pid == Some(pid) {
            out.extend_from_slice(payload);
        }
    }
    out
}

impl SoundCloudProvider {
    /// SoundCloud's own "related tracks" feed for a given track. Used by the
    /// discovery engine as the primary candidate source for niche electronic
    /// where ListenBrainz' similarity graph has no coverage.
    pub async fn related_tracks(&self, uri: &TrackUri, limit: u32) -> ProviderResult<Vec<Track>> {
        let id = track_id_from_uri(uri)?;
        let limit = limit.clamp(1, 50);
        self.with_client_id(|cid| {
            let url = format!("{SC_API}/tracks/{id}/related?limit={limit}&client_id={cid}");
            async move {
                let raw: ScSearchResp = self.fetch_json(&url).await?;
                Ok(raw.collection.into_iter().map(sc_to_track).collect())
            }
        })
        .await
    }

    /// SoundCloud chart rows used by the Aegis-style Explore/Home shelves.
    /// `genre` is the slug after `soundcloud:genres:` (e.g. `electronic`,
    /// `all-music`).
    pub async fn genre_chart(&self, genre: &str, limit: u32) -> ProviderResult<Vec<Track>> {
        let limit = limit.clamp(1, 50);
        let genre = url::form_urlencoded::byte_serialize(genre.as_bytes()).collect::<String>();
        self.with_client_id(|cid| {
            let url = format!(
                "{SC_API}/charts?kind=trending&genre=soundcloud%3Agenres%3A{genre}&limit={limit}&client_id={cid}"
            );
            async move {
                let raw: ScChartsResp = self.fetch_json(&url).await?;
                Ok(raw
                    .collection
                    .into_iter()
                    .filter_map(|item| item.track)
                    .map(sc_to_track)
                    .collect())
            }
        })
        .await
    }

    /// Most-recent uploads for a SoundCloud user/artist. Aegis uses this for
    /// "New from your artists"; nira also reuses it for artist top tracks.
    pub async fn user_tracks(&self, uri: &ArtistUri, limit: u32) -> ProviderResult<Vec<Track>> {
        let id = user_id_from_uri(uri)?;
        let limit = limit.clamp(1, 50);
        self.with_client_id(|cid| {
            let url = format!("{SC_API}/users/{id}/tracks?limit={limit}&client_id={cid}");
            async move {
                let raw: ScSearchResp = self.fetch_json(&url).await?;
                Ok(raw.collection.into_iter().map(sc_to_track).collect())
            }
        })
        .await
    }
}

// ── SoundCloud API shapes (minimal) ─────────────────────────────────────────

#[derive(Deserialize)]
struct ScSearchResp {
    collection: Vec<ScTrack>,
}

#[derive(Deserialize)]
struct ScChartsResp {
    collection: Vec<ScChartItem>,
}

#[derive(Deserialize)]
struct ScChartItem {
    #[serde(default)]
    track: Option<ScTrack>,
}

#[derive(Deserialize)]
struct ScTrack {
    id: u64,
    title: String,
    user: ScUser,
    #[serde(rename = "duration")]
    duration_ms: u64,
    #[serde(default)]
    artwork_url: Option<String>,
    media: ScMedia,
}

#[derive(Deserialize)]
struct ScUser {
    id: u64,
    username: String,
}

#[derive(Deserialize)]
struct ScMedia {
    transcodings: Vec<ScTranscoding>,
}

#[derive(Deserialize, Clone)]
struct ScTranscoding {
    url: String,
    #[allow(dead_code)]
    quality: String,
    format: ScFormat,
}

#[derive(Deserialize, Clone)]
struct ScFormat {
    mime_type: String,
    protocol: String,
}

#[derive(Deserialize)]
struct ScStreamResp {
    url: String,
}

/// Pick the best transcoding for streaming. Prefers progressive MP3 (single
/// HTTP request, no segment parsing), falls back to HLS so tracks that ship
/// only with HLS variants stay playable. Other protocols (e.g. encrypted
/// HLS) bubble up as `NotAvailable` upstream.
fn pick_stream(transcodings: &[ScTranscoding]) -> Option<&ScTranscoding> {
    transcodings
        .iter()
        .find(|t| {
            t.format.protocol == "progressive" && t.format.mime_type.starts_with("audio/mpeg")
        })
        .or_else(|| transcodings.iter().find(|t| t.format.protocol == "hls"))
}

/// Extract segment URLs from an HLS media playlist, resolving relative paths
/// against the playlist URL's directory. Pulled out as a pure function so
/// the parser is unit-testable without hitting SC.
fn parse_m3u8_segments(m3u8: &str, playlist_url: &str) -> Vec<String> {
    let base = playlist_url
        .rsplit_once('/')
        .map(|(b, _)| b.to_string())
        .unwrap_or_else(|| playlist_url.to_string());
    m3u8.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            if line.starts_with("http://") || line.starts_with("https://") {
                line.to_string()
            } else {
                format!("{base}/{line}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3u8_parses_absolute_segments() {
        let m3u8 = "#EXTM3U\n\
            #EXT-X-VERSION:3\n\
            #EXTINF:10.0,\n\
            https://cf.example/seg/0.ts\n\
            #EXTINF:10.0,\n\
            https://cf.example/seg/1.ts\n\
            #EXT-X-ENDLIST\n";
        let out = parse_m3u8_segments(m3u8, "https://cf.example/list.m3u8");
        assert_eq!(
            out,
            vec![
                "https://cf.example/seg/0.ts".to_string(),
                "https://cf.example/seg/1.ts".to_string(),
            ]
        );
    }

    #[test]
    fn m3u8_parses_relative_segments_against_base() {
        let m3u8 = "#EXTM3U\n\
            #EXTINF:10.0,\n\
            seg-0.ts\n\
            #EXTINF:10.0,\n\
            seg-1.ts\n";
        let out = parse_m3u8_segments(m3u8, "https://cf.example/playlist/list.m3u8");
        assert_eq!(
            out,
            vec![
                "https://cf.example/playlist/seg-0.ts".to_string(),
                "https://cf.example/playlist/seg-1.ts".to_string(),
            ]
        );
    }

    #[test]
    fn m3u8_skips_blank_and_comment_lines() {
        let m3u8 = "#EXTM3U\n\n\
            #EXT-X-TARGETDURATION:10\n\
            seg.ts\n\
            \n\
            #EXT-X-ENDLIST\n";
        let out = parse_m3u8_segments(m3u8, "https://cf/list.m3u8");
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with("/seg.ts"));
    }

    #[test]
    fn pick_stream_prefers_progressive_falls_back_to_hls() {
        let hls_only = vec![ScTranscoding {
            url: "h".into(),
            quality: "sq".into(),
            format: ScFormat {
                mime_type: "audio/mpeg".into(),
                protocol: "hls".into(),
            },
        }];
        let mixed = vec![
            ScTranscoding {
                url: "h".into(),
                quality: "sq".into(),
                format: ScFormat {
                    mime_type: "audio/mpeg".into(),
                    protocol: "hls".into(),
                },
            },
            ScTranscoding {
                url: "p".into(),
                quality: "sq".into(),
                format: ScFormat {
                    mime_type: "audio/mpeg".into(),
                    protocol: "progressive".into(),
                },
            },
        ];
        assert_eq!(pick_stream(&hls_only).unwrap().url, "h");
        assert_eq!(pick_stream(&mixed).unwrap().url, "p");
    }
}

fn sc_to_track(sc: ScTrack) -> Track {
    Track {
        uri: TrackUri(format!("soundcloud:track:{}", sc.id)),
        provider: ProviderId::SoundCloud,
        title: clean_sc_title(sc.title),
        artists: vec![ArtistRef {
            uri: ArtistUri(format!("soundcloud:user:{}", sc.user.id)),
            name: sc.user.username,
        }],
        album: None::<provider_api::AlbumRef>,
        duration: Duration::from_millis(sc.duration_ms),
        cover_url: sc.artwork_url.map(upgrade_artwork),
        mbid: None,
        added_at: None,
    }
}

/// SC users sometimes upload tracks where the title is the original
/// filename — `Foo Bar-<md5>.mp3` or `Track.flac`. Trim a trailing audio
/// extension and an immediately preceding hex content-hash so the UI
/// shows what a human would call the song. Conservative: only strips
/// when the suffix unambiguously matches the filename pattern, so
/// genuine titles like `Yesterday` or `Beatles - Yesterday` pass through.
fn clean_sc_title(raw: String) -> String {
    const EXTS: &[&str] = &["mp3", "m4a", "wav", "flac", "ogg", "aac", "opus", "wma"];
    let mut t = raw.trim().to_string();

    let lower = t.to_lowercase();
    for ext in EXTS {
        let suffix = format!(".{ext}");
        if lower.ends_with(&suffix) {
            t.truncate(t.len() - suffix.len());
            break;
        }
    }

    // Strip a trailing `-<hash>` if the hash looks like a content
    // fingerprint (≥16 hex chars). 16 is shorter than md5/sha1 but the
    // false-positive surface for that pattern in real titles is empty.
    if let Some(dash) = t.rfind('-') {
        let tail = &t[dash + 1..];
        if tail.len() >= 16 && tail.chars().all(|c| c.is_ascii_hexdigit()) {
            t.truncate(dash);
        }
    }

    t.trim().to_string()
}

#[cfg(test)]
mod clean_title_tests {
    use super::clean_sc_title;

    #[test]
    fn strips_hash_and_extension() {
        assert_eq!(
            clean_sc_title(
                "Gutes Herz Boyka-PLIESTERBECKEREI-e51e892dfc09f6929e6e0b93de6b0c90.mp3".into()
            ),
            "Gutes Herz Boyka-PLIESTERBECKEREI"
        );
    }

    #[test]
    fn strips_extension_only() {
        assert_eq!(clean_sc_title("Track Name.flac".into()), "Track Name");
    }

    #[test]
    fn leaves_clean_titles_alone() {
        assert_eq!(clean_sc_title("Yesterday".into()), "Yesterday");
        assert_eq!(
            clean_sc_title("Beatles - Yesterday".into()),
            "Beatles - Yesterday"
        );
        // Short suffix that happens to be hex isn't a content-hash.
        assert_eq!(clean_sc_title("Track-abc123".into()), "Track-abc123");
    }
}

// Bump the URL suffix from `-large` (100×100) to `-t500x500` (500×500).
// Otherwise everything looks like postage stamps in the UI.
fn upgrade_artwork(url: String) -> String {
    url.replace("-large.jpg", "-t500x500.jpg")
        .replace("-large.png", "-t500x500.png")
}
