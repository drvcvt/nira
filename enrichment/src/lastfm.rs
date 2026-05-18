//! Last.fm `track.getSimilar` similarity lookups.
//!
//! Uses a single app-owned API key (no user login, no token). The endpoint
//! returns a ranked list of similar tracks with a normalised `match` score
//! in [0.0, 1.0]. Empty if Last.fm has no neighbourhood for the seed.
//!
//! Key sourcing: `AppConfig.lastfm_api_key` → env `NIRA_LASTFM_API_KEY` →
//! none. Discovery skips this source silently when no key is configured.

use serde::Deserialize;

use crate::{EnrichmentClient, EnrichmentError, EnrichmentResult};

const LASTFM_API: &str = "https://ws.audioscrobbler.com/2.0";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastFmSimilar {
    pub title: String,
    pub artist: String,
    /// Normalised in [0.0, 1.0]. Higher = closer neighbour.
    pub score: f32,
}

impl EnrichmentClient {
    /// Returns similar tracks. Empty when no key is configured or Last.fm
    /// has nothing for this seed; Err only on real network/malformed.
    pub async fn lastfm_similar_tracks(
        &self,
        api_key: Option<&str>,
        artist: &str,
        title: &str,
        limit: u32,
    ) -> EnrichmentResult<Vec<LastFmSimilar>> {
        let Some(key) = api_key.filter(|k| !k.trim().is_empty()) else {
            return Ok(Vec::new());
        };
        let cache_key = format!("lastfm:similar:{artist}|{title}|{limit}");
        if let Some(cached) = self.cache().get(&cache_key)
            && let Ok(parsed) = serde_json::from_str::<Vec<LastFmSimilar>>(&cached)
        {
            return Ok(parsed);
        }
        let url = format!(
            "{LASTFM_API}/?method=track.getsimilar&artist={a}&track={t}&limit={limit}&api_key={k}&format=json",
            a = url::form_urlencoded::byte_serialize(artist.as_bytes()).collect::<String>(),
            t = url::form_urlencoded::byte_serialize(title.as_bytes()).collect::<String>(),
            k = url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>(),
        );
        let resp = self
            .http()
            .get(&url)
            .send()
            .await
            .map_err(|e| EnrichmentError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let snippet = resp.text().await.unwrap_or_default();
            return Err(EnrichmentError::Network(format!(
                "Last.fm track.getsimilar -> {status}: {}",
                snippet.chars().take(400).collect::<String>()
            )));
        }
        let raw: LastFmResp = resp
            .json()
            .await
            .map_err(|e| EnrichmentError::Malformed(e.to_string()))?;
        let mapped = parse_similar_tracks(raw);
        let serialised = serde_json::to_string(&mapped).unwrap_or_default();
        self.cache().put(cache_key, serialised);
        Ok(mapped)
    }
}

fn parse_similar_tracks(raw: LastFmResp) -> Vec<LastFmSimilar> {
    let Some(group) = raw.similartracks else {
        return Vec::new();
    };
    group
        .track
        .into_iter()
        .filter_map(|t| {
            let score = t.match_score.as_f32().clamp(0.0, 1.0);
            let title = t.name?;
            let artist = t.artist.and_then(|a| a.name)?;
            Some(LastFmSimilar {
                title,
                artist,
                score,
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct LastFmResp {
    #[serde(default)]
    similartracks: Option<SimilarTracksGroup>,
}

#[derive(Deserialize)]
struct SimilarTracksGroup {
    #[serde(default)]
    track: Vec<RawTrack>,
}

#[derive(Deserialize)]
struct RawTrack {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    artist: Option<RawArtist>,
    #[serde(default, rename = "match")]
    match_score: MatchScore,
}

#[derive(Deserialize)]
struct RawArtist {
    #[serde(default)]
    name: Option<String>,
}

/// Last.fm flips between number and string forms for `match`. Tolerate both.
#[derive(Default, Deserialize)]
#[serde(untagged)]
enum MatchScore {
    Num(f32),
    Str(String),
    #[default]
    Missing,
}

impl MatchScore {
    fn as_f32(&self) -> f32 {
        match self {
            MatchScore::Num(n) => *n,
            MatchScore::Str(s) => s.parse().unwrap_or(0.0),
            MatchScore::Missing => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_number_match() {
        let raw: LastFmResp = serde_json::from_str(
            r#"{"similartracks":{"track":[{"name":"Heroes","artist":{"name":"Bowie"},"match":0.91}]}}"#,
        )
        .unwrap();
        let out = parse_similar_tracks(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Heroes");
        assert_eq!(out[0].artist, "Bowie");
        assert!((out[0].score - 0.91).abs() < 1e-3);
    }

    #[test]
    fn parses_string_match() {
        let raw: LastFmResp = serde_json::from_str(
            r#"{"similartracks":{"track":[{"name":"X","artist":{"name":"Y"},"match":"0.5"}]}}"#,
        )
        .unwrap();
        let out = parse_similar_tracks(raw);
        assert_eq!(out[0].score, 0.5);
    }

    #[test]
    fn empty_when_no_group() {
        let raw: LastFmResp = serde_json::from_str("{}").unwrap();
        assert!(parse_similar_tracks(raw).is_empty());
    }

    #[test]
    fn skips_entries_missing_artist_or_title() {
        let raw: LastFmResp = serde_json::from_str(
            r#"{"similartracks":{"track":[{"match":1.0},{"name":"X","artist":{"name":"Y"},"match":1.0}]}}"#,
        )
        .unwrap();
        let out = parse_similar_tracks(raw);
        assert_eq!(out.len(), 1);
    }
}
