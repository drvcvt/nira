//! MusicBrainz recording search.
//!
//! Endpoint: `https://musicbrainz.org/ws/2/recording/?query=…&fmt=json&limit=…`.
//! Rate limit is one request per second per IP for anonymous clients —
//! we cache aggressively to stay under it on a typical discovery flow
//! (one MB call per seed, none per candidate).

use serde::Deserialize;

use crate::{EnrichmentClient, EnrichmentError, EnrichmentResult};

const MB_API: &str = "https://musicbrainz.org/ws/2";

#[derive(Debug, Clone)]
pub struct MbRecording {
    pub mbid: String,
    pub title: String,
    pub artist: String,
    pub length_ms: Option<u64>,
    /// MusicBrainz internal score (0–100). Higher = better text match.
    pub score: u32,
}

impl EnrichmentClient {
    /// Search for a recording matching (artist, title). Returns ranked hits
    /// (best first). Empty on no match — callers decide whether to fall
    /// back to "title-only" or fail.
    pub async fn mb_search_recording(
        &self,
        artist: &str,
        title: &str,
        limit: u32,
    ) -> EnrichmentResult<Vec<MbRecording>> {
        let key = format!("mb:recording:{artist}|{title}|{limit}");
        if let Some(cached) = self.cache().get(&key)
            && let Ok(parsed) = serde_json::from_str::<Vec<MbRecording>>(&cached)
        {
            return Ok(parsed);
        }

        // MusicBrainz expects Lucene-style queries. Escape what we send by
        // wrapping each term in quotes — sloppy but safe for casual user
        // input (no special-char support, no boolean ops needed). Handles
        // partial input: if only one side is set we narrow on that field
        // alone, so callers can use this as an artist-only search too.
        let query = match (artist.trim().is_empty(), title.trim().is_empty()) {
            (true, true) => return Ok(Vec::new()),
            (true, false) => format!("recording:\"{}\"", escape_lucene(title)),
            (false, true) => format!("artist:\"{}\"", escape_lucene(artist)),
            (false, false) => format!(
                "artist:\"{}\" AND recording:\"{}\"",
                escape_lucene(artist),
                escape_lucene(title)
            ),
        };
        let url = format!(
            "{MB_API}/recording/?query={q}&fmt=json&limit={limit}",
            q = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
        );

        let raw: MbRecordingResp = self
            .http()
            .get(&url)
            .send()
            .await
            .map_err(|e| EnrichmentError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| EnrichmentError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| EnrichmentError::Malformed(e.to_string()))?;

        let mapped: Vec<MbRecording> = raw
            .recordings
            .into_iter()
            .map(|r| MbRecording {
                mbid: r.id,
                title: r.title,
                artist: r
                    .artist_credit
                    .into_iter()
                    .map(|a| a.name)
                    .collect::<Vec<_>>()
                    .join(", "),
                length_ms: r.length,
                score: r.score.unwrap_or(0),
            })
            .collect();

        let serialised = serde_json::to_string(&mapped).unwrap_or_default();
        self.cache().put(key, serialised);
        Ok(mapped)
    }
}

// Implement Serialize on MbRecording so the cache layer can round-trip them
// through JSON without an extra DTO.
impl serde::Serialize for MbRecording {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = ser.serialize_struct("MbRecording", 5)?;
        s.serialize_field("mbid", &self.mbid)?;
        s.serialize_field("title", &self.title)?;
        s.serialize_field("artist", &self.artist)?;
        s.serialize_field("length_ms", &self.length_ms)?;
        s.serialize_field("score", &self.score)?;
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for MbRecording {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            mbid: String,
            title: String,
            artist: String,
            length_ms: Option<u64>,
            score: u32,
        }
        let r = Raw::deserialize(de)?;
        Ok(Self {
            mbid: r.mbid,
            title: r.title,
            artist: r.artist,
            length_ms: r.length_ms,
            score: r.score,
        })
    }
}

fn escape_lucene(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' | '\\' => format!("\\{c}"),
            _ => c.to_string(),
        })
        .collect()
}

#[derive(Deserialize)]
struct MbRecordingResp {
    #[serde(default)]
    recordings: Vec<MbRawRecording>,
}

#[derive(Deserialize)]
struct MbRawRecording {
    id: String,
    title: String,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<MbArtistCredit>,
    #[serde(default)]
    length: Option<u64>,
    #[serde(default)]
    score: Option<u32>,
}

#[derive(Deserialize)]
struct MbArtistCredit {
    name: String,
}
