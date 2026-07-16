//! Cross-provider "same recording" matching — used by the FLAC-first queue
//! swap and the album page's "in library" indicators.
//!
//! Strict on purpose: a wrong match plays the wrong audio, so we only accept
//! candidates whose normalized first artist AND title are equal, and whose
//! durations are both known AND agree within 3 s — an unverifiable duration
//! must not confirm a swap. Live versions, remixes and karaoke covers differ
//! in at least one of those.

use provider_api::Track;

/// Lowercase, strip everything non-alphanumeric, collapse whitespace.
/// "Mystic Dream (feat. X)" and "mystic dream feat x" compare equal.
/// Unicode-aware: Cyrillic/CJK titles keep their letters — ASCII-only
/// stripping used to normalize them to "" and made every guard vacuous.
pub fn match_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `(first artist, title)` normalized — the identity we match tracks on.
pub fn track_match_key(track: &Track) -> (String, String) {
    let artist = track
        .artists
        .first()
        .map(|a| match_key(&a.name))
        .unwrap_or_default();
    (artist, match_key(&track.title))
}

/// First candidate that is strictly the same recording as `target`.
pub fn find_strict_match<'a>(target: &Track, candidates: &'a [Track]) -> Option<&'a Track> {
    let (t_artist, t_title) = track_match_key(target);
    if t_title.is_empty() {
        return None;
    }
    candidates.iter().find(|c| {
        let (c_artist, c_title) = track_match_key(c);
        if c_title != t_title || c_artist != t_artist {
            return false;
        }
        let (a, b) = (target.duration.as_secs(), c.duration.as_secs());
        // Both durations must be known and agree. A 0 ("unknown") duration
        // used to skip this veto — the hi-res provider tracks without a duration field
        // deserialize to 0, and the veto is the only guard separating
        // same-titled versions (edit vs. re-recording), so unknown = reject.
        a > 0 && b > 0 && a.abs_diff(b) <= 3
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_api::{ArtistRef, ArtistUri, TrackUri};
    use std::time::Duration;

    fn track(artist: &str, title: &str, secs: u64) -> Track {
        Track {
            uri: TrackUri(format!("test:track:{title}")),
            title: title.into(),
            artists: vec![ArtistRef {
                uri: ArtistUri(String::new()),
                name: artist.into(),
            }],
            album: None,
            duration: Duration::from_secs(secs),
            cover_url: None,
            provider: provider_api::ProviderId::the hi-res provider,
            mbid: None,
            added_at: None,
        }
    }

    #[test]
    fn strict_match_rules() {
        let target = track("Shiro Tanaka", "Mystic Dream (feat. Yuki)", 186);
        // Punctuation/case-insensitive hit within duration tolerance.
        let hit = track("shiro tanaka", "mystic dream feat yuki", 188);
        // Same name, but a live version 40 s longer — must not match.
        let live = track("Shiro Tanaka", "Mystic Dream (feat. Yuki)", 226);
        // Different artist — must not match.
        let cover = track("Karaoke Band", "Mystic Dream (feat. Yuki)", 186);

        assert!(find_strict_match(&target, &[live.clone(), cover.clone()]).is_none());
        let candidates = [live, cover, hit.clone()];
        let found = find_strict_match(&target, &candidates).unwrap();
        assert_eq!(found.uri, hit.uri);

        // Unknown duration on either side can't confirm a match — strict
        // means a candidate we can't length-verify is rejected.
        let no_dur = track("Shiro Tanaka", "Mystic Dream (feat. Yuki)", 0);
        assert!(find_strict_match(&no_dur, &[hit.clone()]).is_none());
        assert!(find_strict_match(&hit, &[no_dur]).is_none());
    }

    #[test]
    fn match_key_keeps_non_ascii() {
        // Cyrillic/CJK must survive normalization — ASCII-only stripping
        // normalized these to "" and made every downstream guard vacuous.
        assert_eq!(match_key("Группа крови"), "группа крови");
        assert_eq!(match_key("残酷な天使のテーゼ"), "残酷な天使のテーゼ");
        assert!(!match_key("Кино").is_empty());
        // Pure-symbol titles still normalize to empty — callers must treat
        // an empty key as unmatchable, not as match-everything.
        assert_eq!(match_key("!!!"), "");
    }
}
