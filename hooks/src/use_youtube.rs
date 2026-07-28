//! YouTube preview and MP3 import through the system `yt-dlp`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use serde::Deserialize;

use crate::{PlaylistImport, Track, UseDownloads, UseLocalLibrary, UsePlaylists};

#[derive(Clone, Debug, PartialEq)]
pub struct YouTubePreview {
    pub title: String,
    pub uploader: String,
    pub thumbnail: Option<String>,
    pub duration: Option<u64>,
    url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YouTubePlaylist {
    pub id: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub track_count: usize,
    pub url: String,
}

#[derive(Clone, Copy)]
pub struct UseYouTube {
    pub preview: Signal<Option<YouTubePreview>>,
    pub status: Signal<Option<String>>,
    pub busy: Signal<bool>,
    pub failed: Signal<bool>,
}

impl UseYouTube {
    pub fn preview(&self, raw_url: String) {
        let mut preview = self.preview;
        preview.set(None);
        let url = match youtube_url(&raw_url) {
            Ok(url) => url.to_string(),
            Err(error) => {
                self.fail(error);
                return;
            }
        };

        self.start("Loading YouTube preview…");

        let state = *self;
        spawn_forever(async move {
            let args = [
                "--dump-single-json",
                "--no-playlist",
                "--skip-download",
                "--no-warnings",
                "--",
                &url,
            ]
            .into_iter()
            .map(OsString::from)
            .collect();

            match run_ytdlp(args).await.and_then(successful_output) {
                Ok(output) => match parse_preview(&output.stdout, url) {
                    Ok(item) => {
                        preview.set(Some(item));
                        state.finish("Ready to download.");
                    }
                    Err(error) => state.fail(error),
                },
                Err(error) => state.fail(error),
            }
        });
    }

    pub fn download(
        &self,
        local: UseLocalLibrary,
        downloads: UseDownloads,
        library_root: Option<PathBuf>,
    ) {
        let Some(item) = self.preview.peek().clone() else {
            let message = "Preview a YouTube song first.";
            self.fail(message);
            downloads.fail(format!("YouTube · {message}"));
            return;
        };
        let Some(root) = library_root else {
            let message = "Set a music folder in Settings → Library first.";
            self.fail(message);
            downloads.fail(format!("YouTube · {message}"));
            return;
        };

        let title = item.title.clone();
        let start_message = download_start_message(&title);
        self.start(start_message.clone());
        downloads.start(start_message);
        let output_dir = root.join("YouTube");
        let state = *self;
        spawn_forever(async move {
            let args = vec![
                "--no-playlist".into(),
                "--extract-audio".into(),
                "--audio-format".into(),
                "mp3".into(),
                "--audio-quality".into(),
                "0".into(),
                "--embed-metadata".into(),
                "--embed-thumbnail".into(),
                "--convert-thumbnails".into(),
                "jpg".into(),
                "--no-progress".into(),
                "--paths".into(),
                output_dir.into_os_string(),
                "--output".into(),
                "%(uploader).40B - %(title).160B [%(id)s].%(ext)s".into(),
                "--print".into(),
                "after_move:filepath".into(),
                "--".into(),
                item.url.into(),
            ];

            match run_ytdlp(args).await.and_then(successful_output) {
                Ok(_) => {
                    local.rescan();
                    state.finish("Saved to your YouTube downloads.");
                    downloads.finish(download_saved_message(&title));
                }
                Err(error) => {
                    state.fail(error.clone());
                    downloads.fail(format!("YouTube · Download failed: {error}"));
                }
            }
        });
    }

    pub async fn inspect_playlist(&self, raw: String) -> Result<YouTubePlaylist, String> {
        let url = youtube_url(&raw)?.to_string();
        let output = run_ytdlp(vec![
            "--dump-single-json".into(),
            "--flat-playlist".into(),
            "--yes-playlist".into(),
            "--skip-download".into(),
            "--no-warnings".into(),
            "--".into(),
            url.clone().into(),
        ])
        .await
        .and_then(successful_output)?;
        parse_playlist(&output.stdout, url)
    }

    pub fn import_playlist(
        &self,
        playlist: YouTubePlaylist,
        local: UseLocalLibrary,
        playlists: UsePlaylists,
        downloads: UseDownloads,
        library_root: Option<PathBuf>,
    ) {
        let Some(root) = library_root else {
            let message = "Set a music folder in Settings → Library first.";
            self.fail(message);
            downloads.fail(format!("YouTube · {message}"));
            return;
        };
        let title = playlist.title.clone();
        let output_dir = root.join("YouTube").join(&playlist.id);
        self.start(format!("Downloading YouTube playlist “{title}”…"));
        downloads.start(format!("YouTube · Downloading “{title}”…"));
        let state = *self;
        spawn_forever(async move {
            let args = vec![
                "--yes-playlist".into(),
                "--ignore-errors".into(),
                "--extract-audio".into(),
                "--audio-format".into(),
                "mp3".into(),
                "--audio-quality".into(),
                "0".into(),
                "--embed-metadata".into(),
                "--embed-thumbnail".into(),
                "--convert-thumbnails".into(),
                "jpg".into(),
                "--quiet".into(),
                "--no-warnings".into(),
                "--paths".into(),
                output_dir.clone().into_os_string(),
                "--output".into(),
                "%(playlist_index)05d - %(uploader).40B - %(title).160B [%(id)s].%(ext)s".into(),
                "--print".into(),
                "after_move:filepath".into(),
                "--".into(),
                playlist.url.clone().into(),
            ];
            let result: Result<(usize, usize, usize), String> = async {
                let output = run_ytdlp(args).await?;
                let paths = output_paths(&output)?;
                let scan_dir = output_dir.clone();
                let covers_dir = config::AppConfig::covers_cache_dir();
                let scanned = tokio::task::spawn_blocking(move || {
                    provider_local::scan(&scan_dir, covers_dir.as_deref())
                })
                .await
                .map_err(|error| format!("Local library scan failed: {error}"))?;
                let tracks = order_downloaded_tracks(&paths, scanned);
                if tracks.is_empty() {
                    return Err("Downloaded files could not be added to the local library.".into());
                }
                let imported_tracks = tracks.len();
                let skipped = playlist.track_count.saturating_sub(imported_tracks);
                let imported = playlists.import_external(
                    "youtube",
                    vec![PlaylistImport {
                        source_id: playlist.id.clone(),
                        name: playlist.title.clone(),
                        tracks,
                    }],
                );
                local.rescan();
                Ok((imported, imported_tracks, skipped))
            }
            .await;

            match result {
                Ok((imported, downloaded, skipped)) => {
                    let existing = if imported == 0 {
                        " Already imported."
                    } else {
                        ""
                    };
                    let skipped = if skipped > 0 {
                        format!(" {skipped} unavailable entries skipped.")
                    } else {
                        String::new()
                    };
                    let message =
                        format!("Imported {downloaded} YouTube tracks.{existing}{skipped}");
                    state.finish(&message);
                    downloads.finish(format!("YouTube · {message}"));
                }
                Err(error) => {
                    state.fail(error.clone());
                    downloads.fail(format!("YouTube · Import failed: {error}"));
                }
            }
        });
    }

    fn start(&self, message: impl Into<String>) {
        let mut status = self.status;
        let mut busy = self.busy;
        let mut failed = self.failed;
        status.set(Some(message.into()));
        busy.set(true);
        failed.set(false);
    }

    fn finish(&self, message: impl Into<String>) {
        let mut status = self.status;
        let mut busy = self.busy;
        let mut failed = self.failed;
        status.set(Some(message.into()));
        busy.set(false);
        failed.set(false);
    }

    fn fail(&self, message: impl Into<String>) {
        let mut status = self.status;
        let mut busy = self.busy;
        let mut failed = self.failed;
        status.set(Some(message.into()));
        busy.set(false);
        failed.set(true);
    }
}

pub fn install_youtube() {
    let state = use_hook(|| UseYouTube {
        preview: Signal::new_in_scope(None, ScopeId::ROOT),
        status: Signal::new_in_scope(None, ScopeId::ROOT),
        busy: Signal::new_in_scope(false, ScopeId::ROOT),
        failed: Signal::new_in_scope(false, ScopeId::ROOT),
    });
    use_context_provider(move || state);
}

pub fn use_youtube() -> UseYouTube {
    use_context::<UseYouTube>()
}

fn youtube_url(raw: &str) -> Result<reqwest::Url, String> {
    let url =
        reqwest::Url::parse(raw.trim()).map_err(|_| "Paste a valid YouTube URL.".to_string())?;
    let host = url.host_str().unwrap_or_default();
    let allowed = matches!(
        host,
        "youtube.com"
            | "www.youtube.com"
            | "m.youtube.com"
            | "music.youtube.com"
            | "youtu.be"
            | "www.youtube-nocookie.com"
    );
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) || !allowed {
        return Err("Paste an HTTPS youtube.com or youtu.be URL.".into());
    }
    Ok(url)
}

fn download_start_message(title: &str) -> String {
    format!("YouTube · Downloading “{title}” as MP3…")
}

fn download_saved_message(title: &str) -> String {
    format!("YouTube · Saved “{title}”.")
}

async fn run_ytdlp(args: Vec<OsString>) -> Result<Output, String> {
    tokio::task::spawn_blocking(move || {
        Command::new("yt-dlp").args(args).output().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "yt-dlp is not installed or not in PATH.".to_string()
            } else {
                format!("Could not start yt-dlp: {error}")
            }
        })
    })
    .await
    .map_err(|error| format!("yt-dlp task failed: {error}"))?
}

fn successful_output(output: Output) -> Result<Output, String> {
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("yt-dlp failed.")
        .trim()
        .to_string())
}

fn output_paths(output: &Output) -> Result<Vec<PathBuf>, String> {
    let paths: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .collect();
    if !paths.is_empty() {
        return Ok(paths);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("yt-dlp did not produce any audio files.")
        .trim()
        .to_string())
}

fn order_downloaded_tracks(paths: &[PathBuf], tracks: Vec<Track>) -> Vec<Track> {
    let mut by_uri: std::collections::HashMap<String, Track> = tracks
        .into_iter()
        .map(|track| (track.uri.0.clone(), track))
        .collect();
    paths
        .iter()
        .filter_map(|path| by_uri.remove(&provider_local::track_uri(path)))
        .collect()
}

#[derive(Deserialize)]
struct PreviewJson {
    title: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    thumbnail: Option<String>,
    duration: Option<f64>,
}

#[derive(Deserialize)]
struct PlaylistJson {
    #[serde(rename = "_type")]
    kind: Option<String>,
    id: Option<String>,
    title: Option<String>,
    thumbnail: Option<String>,
    #[serde(default)]
    entries: Vec<serde_json::Value>,
}

fn parse_playlist(bytes: &[u8], source_url: String) -> Result<YouTubePlaylist, String> {
    let data: PlaylistJson = serde_json::from_slice(bytes)
        .map_err(|error| format!("Could not read yt-dlp playlist: {error}"))?;
    if data.kind.as_deref() != Some("playlist") {
        return Err("Paste a YouTube playlist URL, not a single video.".into());
    }
    let id = data
        .id
        .filter(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or_else(|| "YouTube playlist has no safe id.".to_string())?;
    let title = data
        .title
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| "YouTube playlist has no title.".to_string())?;
    if data.entries.is_empty() {
        return Err("This YouTube playlist has no visible entries.".into());
    }
    Ok(YouTubePlaylist {
        id,
        title,
        cover_url: data.thumbnail,
        track_count: data.entries.len(),
        url: source_url,
    })
}

fn parse_preview(bytes: &[u8], url: String) -> Result<YouTubePreview, String> {
    let data: PreviewJson = serde_json::from_slice(bytes)
        .map_err(|error| format!("Could not read yt-dlp preview: {error}"))?;
    let title = data
        .title
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "YouTube preview has no title.".to_string())?;

    Ok(YouTubePreview {
        title,
        uploader: data
            .uploader
            .or(data.channel)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "YouTube".to_string()),
        thumbnail: data.thumbnail,
        duration: data
            .duration
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value.round() as u64),
        url,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::{ProviderId, Track, TrackUri};

    use super::{
        download_saved_message, download_start_message, order_downloaded_tracks, parse_playlist,
        youtube_url,
    };

    #[test]
    fn accepts_only_https_youtube_urls() {
        assert!(youtube_url("https://youtu.be/dQw4w9WgXcQ").is_ok());
        assert!(youtube_url("https://music.youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
        assert!(youtube_url("http://youtube.com/watch?v=x").is_err());
        assert!(youtube_url("https://youtube.com.evil.test/watch?v=x").is_err());
        assert!(youtube_url("https://youtube.com:444/playlist?list=PLx").is_err());
    }

    #[test]
    fn parses_a_youtube_playlist_preview() {
        let json = br#"{
            "_type": "playlist",
            "id": "PLabc_123",
            "title": "Road trip",
            "thumbnail": "https://i.ytimg.com/list.jpg",
            "entries": [{"id":"a"}, {"id":"b"}]
        }"#;
        let playlist = parse_playlist(
            json,
            "https://www.youtube.com/playlist?list=PLabc_123".into(),
        )
        .expect("playlist parses");

        assert_eq!(playlist.id, "PLabc_123");
        assert_eq!(playlist.title, "Road trip");
        assert_eq!(playlist.track_count, 2);
    }

    #[test]
    fn rejects_single_video_as_playlist_import() {
        let json = br#"{
            "_type": "video",
            "id": "abc",
            "title": "One video"
        }"#;
        assert!(parse_playlist(json, "https://youtu.be/abc".into()).is_err());
    }

    #[test]
    fn downloaded_tracks_follow_ytdlp_output_order() {
        let first = PathBuf::from("/music/second.mp3");
        let second = PathBuf::from("/music/first.mp3");
        let tracks = vec![
            local_test_track(&second, "First"),
            local_test_track(&first, "Second"),
        ];

        let ordered = order_downloaded_tracks(&[first, second], tracks);
        assert_eq!(
            ordered
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Second", "First"]
        );
    }

    #[test]
    fn download_messages_name_youtube_and_the_track() {
        assert_eq!(
            download_start_message("A Song"),
            "YouTube · Downloading “A Song” as MP3…"
        );
        assert_eq!(
            download_saved_message("A Song"),
            "YouTube · Saved “A Song”."
        );
    }

    fn local_test_track(path: &Path, title: &str) -> Track {
        Track {
            uri: TrackUri(provider_local::track_uri(path)),
            provider: ProviderId::Local,
            title: title.into(),
            artists: Vec::new(),
            album: None,
            duration: std::time::Duration::ZERO,
            cover_url: None,
            mbid: None,
            added_at: None,
        }
    }
}
