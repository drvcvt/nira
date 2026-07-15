//! Local-file library scanner.
//!
//! Walks the user's configured music folder, reads tags + duration via
//! `lofty`, and produces [`provider_api::Track`]s tagged [`ProviderId::Local`].
//!
//! There is deliberately **no** `Provider` trait impl here: local files need
//! no search / auth / stream-resolution. The queue recovers the file path
//! straight out of the [`TrackUri`] (`local:track:<absolute-path>`) and hands
//! it to the audio engine, so this crate is only the scan half.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lofty::picture::{MimeType, PictureType};
use lofty::prelude::*;
use provider_api::{AlbumRef, AlbumUri, ArtistRef, ArtistUri, ProviderId, Track, TrackUri};

/// Extensions we can actually decode — matches the rodio feature set wired in
/// the workspace manifest (flac / mp3 / mp4+aac / vorbis / wav).
const AUDIO_EXTS: &[&str] = &["flac", "mp3", "m4a", "mp4", "aac", "ogg", "oga", "wav"];

const URI_PREFIX: &str = "local:track:";

/// Build the playback URI for a local file. The queue strips [`URI_PREFIX`]
/// back off to recover the path.
pub fn track_uri(path: &Path) -> String {
    format!("{URI_PREFIX}{}", path.display())
}

/// Recover the filesystem path from a `local:track:` URI, if it is one.
pub fn path_from_uri(uri: &str) -> Option<&Path> {
    uri.strip_prefix(URI_PREFIX).map(Path::new)
}

/// Total on-disk size of the given local tracks, in bytes. Stats each file
/// once; unreadable files count as 0. Call from a blocking context alongside
/// [`scan`] — it's cheap (one metadata syscall per track) but still IO.
pub fn total_size_bytes(tracks: &[Track]) -> u64 {
    tracks
        .iter()
        .filter_map(|t| path_from_uri(&t.uri.0))
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// Recursively scan `root` for decodable audio files and turn each into a
/// [`Track`]. Untagged files fall back to the filename for a title and
/// "Unknown Artist". The result is sorted artist → album → disc → track so
/// albums queue up in playing order. Unreadable files/dirs are skipped and
/// logged, never fatal — one broken file must not sink the whole library.
///
/// When `covers_dir` is given, embedded album art is extracted once per
/// album into that directory and every track of the album gets
/// `cover_url = "/covers/<file>"` — the desktop shell serves that path via
/// its asset handler.
pub fn scan(root: &Path, covers_dir: Option<&Path>) -> Vec<Track> {
    let mut files = Vec::new();
    collect_files(root, &mut files);
    if let Some(dir) = covers_dir {
        let _ = std::fs::create_dir_all(dir);
    }
    // album key → served cover URL. Filled while reading files, applied in a
    // second pass — the first file of an album read from disk isn't
    // necessarily the one carrying the art.
    let mut covers: HashMap<String, String> = HashMap::new();
    let mut keyed: Vec<(SortKey, String, Track)> = files
        .iter()
        .filter_map(|p| track_from_file(p, covers_dir, &mut covers))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let tracks: Vec<Track> = keyed
        .into_iter()
        .map(|(_, album_key, mut track)| {
            track.cover_url = covers.get(&album_key).cloned();
            track
        })
        .collect();
    tracing::info!(root = %root.display(), count = tracks.len(), "local scan complete");
    tracks
}

/// (artist, album, disc, track, title) — all lowercased so the sort is
/// case-insensitive. Carried alongside the `Track` because `Track` itself
/// has no track-number field.
type SortKey = (String, String, u32, u32, String);

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(path = %dir.display(), error = %e, "local scan: cannot read dir");
            return;
        }
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        // ponytail: symlinks skipped (file_type doesn't follow them) — avoids
        // directory-loop bookkeeping. Add loop-safe following if users need it.
        if ft.is_dir() {
            collect_files(&path, out);
        } else if ft.is_file() && has_audio_ext(&path) {
            out.push(path);
        }
    }
}

fn has_audio_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn track_from_file(
    path: &Path,
    covers_dir: Option<&Path>,
    covers: &mut HashMap<String, String>,
) -> Option<(SortKey, String, Track)> {
    let tagged = match lofty::read_from_path(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "local scan: tag read failed");
            return None;
        }
    };
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let duration = tagged.properties().duration();

    let non_empty = |s: String| (!s.trim().is_empty()).then_some(s);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown")
        .to_string();
    let title = tag
        .and_then(|t| t.title())
        .and_then(|c| non_empty(c.to_string()))
        .unwrap_or(stem);
    let artist = tag
        .and_then(|t| t.artist())
        .and_then(|c| non_empty(c.to_string()))
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let album = tag
        .and_then(|t| t.album())
        .and_then(|c| non_empty(c.to_string()));
    let track_no = tag.and_then(|t| t.track()).unwrap_or(0);
    let disc_no = tag.and_then(|t| t.disk()).unwrap_or(0);

    // Artist-scoped album key: album titles alone collide across artists
    // ("Greatest Hits"), and this key doubles as the `local:album:` URI id
    // the album detail page resolves.
    let album_key = format!(
        "{}|{}",
        artist.to_lowercase(),
        album.as_deref().unwrap_or_default().to_lowercase()
    );

    // Harvest embedded art once per album — the first file that actually
    // carries a picture wins. Written to the covers cache, served to the
    // webview under "/covers/…".
    if album.is_some()
        && let Some(dir) = covers_dir
        && !covers.contains_key(&album_key)
        && let Some(t) = tag
        && let Some(pic) = t
            .pictures()
            .iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| t.pictures().first())
    {
        let ext = match pic.mime_type() {
            Some(MimeType::Png) => "png",
            _ => "jpg",
        };
        let name = format!("{:016x}.{ext}", stable_hash(&album_key));
        let file = dir.join(&name);
        let ok = file.exists() || std::fs::write(&file, pic.data()).is_ok();
        if ok {
            covers.insert(album_key.clone(), format!("/covers/{name}"));
        }
    }

    let sort_key: SortKey = (
        artist.to_lowercase(),
        album.as_deref().unwrap_or_default().to_lowercase(),
        disc_no,
        track_no,
        title.to_lowercase(),
    );

    let track = Track {
        uri: TrackUri(track_uri(path)),
        provider: ProviderId::Local,
        title,
        artists: vec![ArtistRef {
            uri: ArtistUri(format!("local:artist:{}", artist.to_lowercase())),
            name: artist,
        }],
        album: album.map(|a| AlbumRef {
            uri: AlbumUri(format!("local:album:{album_key}")),
            title: a,
        }),
        duration,
        cover_url: None, // filled from the covers map in scan()'s second pass
        mbid: None,
        added_at: None,
    };
    Some((sort_key, album_key, track))
}

/// Deterministic-in-process hash for cover filenames. If the std hasher ever
/// changes across Rust releases the covers just re-extract under new names —
/// harmless, the cache directory is disposable.
fn stable_hash(s: &str) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrips_through_the_queue_path() {
        let p = Path::new("/music/Some Artist/01 track.flac");
        let uri = track_uri(p);
        assert_eq!(path_from_uri(&uri), Some(p));
        // A foreign provider URI is rejected.
        assert_eq!(path_from_uri("spotify:track:xyz"), None);
    }

    #[test]
    fn audio_ext_filter_is_case_insensitive() {
        assert!(has_audio_ext(Path::new("x.flac")));
        assert!(has_audio_ext(Path::new("x.FLAC")));
        assert!(has_audio_ext(Path::new("x.Mp3")));
        assert!(!has_audio_ext(Path::new("cover.jpg")));
        assert!(!has_audio_ext(Path::new("noext")));
    }
}
