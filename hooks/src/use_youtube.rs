//! YouTube preview and MP3 import through the system `yt-dlp`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use serde::Deserialize;

use crate::{UseDownloads, UseLocalLibrary};

#[derive(Clone, Debug, PartialEq)]
pub struct YouTubePreview {
    pub title: String,
    pub uploader: String,
    pub thumbnail: Option<String>,
    pub duration: Option<u64>,
    url: String,
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
    if url.scheme() != "https" || !allowed {
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

#[derive(Deserialize)]
struct PreviewJson {
    title: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    thumbnail: Option<String>,
    duration: Option<f64>,
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
    use super::{download_saved_message, download_start_message, youtube_url};

    #[test]
    fn accepts_only_https_youtube_urls() {
        assert!(youtube_url("https://youtu.be/dQw4w9WgXcQ").is_ok());
        assert!(youtube_url("https://music.youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
        assert!(youtube_url("http://youtube.com/watch?v=x").is_err());
        assert!(youtube_url("https://youtube.com.evil.test/watch?v=x").is_err());
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
}
