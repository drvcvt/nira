//! ListenBrainz similarity lookups.
//!
//! Uses the `labs.api.listenbrainz.org/similar-recordings/json` endpoint,
//! which returns up to ~100 neighbouring recordings ranked by co-listening
//! score. The response already embeds title + artist credit, so a single
//! call yields everything discovery needs to fan out to the providers
//! without per-candidate metadata round-trips.
//!
//! The "labs" path is technically experimental, but it's the same endpoint
//! ListenBrainz Radio uses internally and has been stable for a long time.
//! If/when it moves we adapt — encapsulated here.

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::{EnrichmentClient, EnrichmentError, EnrichmentResult};

const LB_LABS: &str = "https://labs.api.listenbrainz.org";
const LB_API: &str = "https://api.listenbrainz.org";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LbSimilar {
    pub mbid: String,
    pub title: String,
    pub artist: String,
    /// Raw co-listening score from ListenBrainz. Higher = closer neighbour.
    /// Scale is algorithm-specific; treat as an ordinal, not a probability.
    pub score: f32,
}

impl EnrichmentClient {
    /// Similar recordings to the given recording MBID. Empty if LB has no
    /// neighbourhood data for that MBID (common for obscure tracks).
    pub async fn lb_similar_recordings(
        &self,
        mbid: &str,
        limit: u32,
    ) -> EnrichmentResult<Vec<LbSimilar>> {
        let key = format!("lb:similar:{mbid}:{limit}");
        if let Some(cached) = self.cache().get(&key)
            && let Ok(parsed) = serde_json::from_str::<Vec<LbSimilar>>(&cached)
        {
            return Ok(parsed);
        }

        // The labs endpoint takes MBID + algorithm enum. LB renames these
        // periodically; if a value 400s, bump the error-snippet length in
        // the handler below to see the full `permitted:` list and pick a
        // current member. As of late 2025 the names use threshold_15,
        // limit_50 and (some variants) top_n_listeners_1000.
        let url = format!(
            "{LB_LABS}/similar-recordings/json?recording_mbids={mbid}\
             &algorithm=session_based_days_7500_session_300_contribution_5_threshold_15_limit_50_skip_30_top_n_listeners_1000"
        );
        let resp = self
            .http()
            .get(&url)
            .send()
            .await
            .map_err(|e| EnrichmentError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(%status, %body, "ListenBrainz similar-recordings failed");
            return Err(EnrichmentError::Network(format!(
                "ListenBrainz similar-recordings -> {status}: {}",
                body.chars().take(600).collect::<String>()
            )));
        }
        let raw: LbResponse = resp
            .json()
            .await
            .map_err(|e| EnrichmentError::Malformed(e.to_string()))?;

        let mapped: Vec<LbSimilar> = raw
            .into_recordings()
            .into_iter()
            .take(limit as usize)
            .map(|r| LbSimilar {
                mbid: r.recording_mbid,
                title: r.recording_name,
                artist: r.artist_credit_name,
                score: r.score,
            })
            .collect();

        let serialised = serde_json::to_string(&mapped).unwrap_or_default();
        self.cache().put(key, serialised);
        Ok(mapped)
    }
}

/// ListenBrainz returns either a flat array or `{ data: [...] }` depending on
/// the endpoint variant. Accept both via a tagged enum.
#[derive(Deserialize)]
#[serde(untagged)]
enum LbResponse {
    Flat(Vec<LbRawRecording>),
    Wrapped { data: Vec<LbRawRecording> },
}

impl LbResponse {
    fn into_recordings(self) -> Vec<LbRawRecording> {
        match self {
            LbResponse::Flat(v) => v,
            LbResponse::Wrapped { data } => data,
        }
    }
}

#[derive(Deserialize)]
struct LbRawRecording {
    recording_mbid: String,
    #[serde(default)]
    recording_name: String,
    #[serde(default)]
    artist_credit_name: String,
    #[serde(default)]
    score: f32,
}

// ── User listens (inbound) ─────────────────────────────────────────────────

/// One row in Home's "Listened lately" feed. Cross-platform: the user might
/// have scrobbled this from Spotify, nira, or another player — `source`
/// captures whatever LB knows about the origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listen {
    pub mbid: Option<String>,
    pub title: String,
    pub artist: String,
    pub listened_at: DateTime<Utc>,
    pub source: Option<String>,
}

#[derive(Deserialize)]
struct LbListensWrapper {
    payload: LbListensPayload,
}

#[derive(Deserialize)]
struct LbListensPayload {
    #[serde(default)]
    listens: Vec<LbRawListen>,
}

#[derive(Deserialize)]
struct LbRawListen {
    #[serde(default)]
    listened_at: u64,
    track_metadata: LbRawTrackMeta,
}

#[derive(Deserialize)]
struct LbRawTrackMeta {
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    additional_info: Option<LbRawAdditional>,
}

#[derive(Deserialize)]
struct LbRawAdditional {
    #[serde(default)]
    recording_mbid: Option<String>,
    #[serde(default)]
    listening_from: Option<String>,
    #[serde(default)]
    music_service: Option<String>,
}

// ── Scrobble (outbound) ────────────────────────────────────────────────────

#[derive(Serialize)]
struct LbListenPayload<'a> {
    listen_type: &'a str,
    payload: Vec<LbListen<'a>>,
}

#[derive(Serialize)]
struct LbListen<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    listened_at: Option<u64>,
    track_metadata: LbTrackMeta<'a>,
}

#[derive(Serialize)]
struct LbTrackMeta<'a> {
    track_name: &'a str,
    artist_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_name: Option<&'a str>,
}

impl EnrichmentClient {
    /// Tell ListenBrainz "this track is being played right now". Should be
    /// called once when a new track starts. No-op if no token is configured.
    pub async fn lb_playing_now(
        &self,
        token: &str,
        title: &str,
        artist: &str,
    ) -> EnrichmentResult<()> {
        if token.trim().is_empty() {
            return Ok(());
        }
        let body = LbListenPayload {
            listen_type: "playing_now",
            payload: vec![LbListen {
                listened_at: None,
                track_metadata: LbTrackMeta {
                    track_name: title,
                    artist_name: artist,
                    release_name: None,
                },
            }],
        };
        self.lb_post_listens(token, &body).await
    }

    /// Submit a permanent "I listened to this" record. Caller is expected to
    /// have already gated on play-time (>= 4 min OR >= 50% of track length)
    /// per the ListenBrainz client convention.
    pub async fn lb_submit_listen(
        &self,
        token: &str,
        title: &str,
        artist: &str,
        listened_at_unix: u64,
    ) -> EnrichmentResult<()> {
        if token.trim().is_empty() {
            return Ok(());
        }
        let body = LbListenPayload {
            listen_type: "single",
            payload: vec![LbListen {
                listened_at: Some(listened_at_unix),
                track_metadata: LbTrackMeta {
                    track_name: title,
                    artist_name: artist,
                    release_name: None,
                },
            }],
        };
        self.lb_post_listens(token, &body).await
    }

    /// Fetch the user's recent listens. Home's "Listened lately" row reads
    /// this. The endpoint is `/1/user/<user>/listens?count=N` and does not
    /// require an auth header for public profiles — listens are public on
    /// ListenBrainz by default.
    pub async fn lb_user_listens(
        &self,
        username: &str,
        limit: u32,
    ) -> EnrichmentResult<Vec<Listen>> {
        let username = username.trim();
        if username.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 100);
        let key = format!("lb:listens:{username}:{limit}");
        if let Some(cached) = self.cache().get(&key)
            && let Ok(parsed) = serde_json::from_str::<Vec<Listen>>(&cached)
        {
            return Ok(parsed);
        }

        let url_username: String =
            url::form_urlencoded::byte_serialize(username.as_bytes()).collect();
        let url = format!("{LB_API}/1/user/{url_username}/listens?count={limit}");
        let resp = self
            .http()
            .get(&url)
            .send()
            .await
            .map_err(|e| EnrichmentError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EnrichmentError::Network(format!(
                "ListenBrainz listens -> {status}: {}",
                body.chars().take(400).collect::<String>()
            )));
        }
        let raw: LbListensWrapper = resp
            .json()
            .await
            .map_err(|e| EnrichmentError::Malformed(e.to_string()))?;

        let listens: Vec<Listen> = raw
            .payload
            .listens
            .into_iter()
            .filter_map(|l| {
                let listened_at = Utc
                    .timestamp_opt(l.listened_at as i64, 0)
                    .single()?;
                Some(Listen {
                    mbid: l
                        .track_metadata
                        .additional_info
                        .as_ref()
                        .and_then(|a| a.recording_mbid.clone()),
                    title: l.track_metadata.track_name,
                    artist: l.track_metadata.artist_name,
                    listened_at,
                    source: l
                        .track_metadata
                        .additional_info
                        .and_then(|a| a.listening_from.or(a.music_service)),
                })
            })
            .collect();

        let serialised = serde_json::to_string(&listens).unwrap_or_default();
        self.cache().put(key, serialised);
        Ok(listens)
    }

    async fn lb_post_listens(
        &self,
        token: &str,
        body: &LbListenPayload<'_>,
    ) -> EnrichmentResult<()> {
        let url = format!("{LB_API}/1/submit-listens");
        let resp = self
            .http()
            .post(&url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|e| EnrichmentError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let snippet = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>();
            return Err(EnrichmentError::Network(format!(
                "ListenBrainz submit-listens -> {status}: {snippet}"
            )));
        }
        Ok(())
    }
}
