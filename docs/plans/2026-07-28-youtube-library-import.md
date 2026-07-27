# YouTube Library Import Implementation Plan

> **For agentic workers:** Implement this plan task-by-task — one task per commit, run each task's test before moving on. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preview a YouTube song and download it as a tagged MP3 into Nira's configured local library from Library → Local.

**Architecture:** A root-installed `use_youtube` hook owns preview/download state so work survives page navigation. It invokes the already-standard `yt-dlp` and `ffmpeg` executables directly without a shell, while the Library page renders one compact import surface and reuses the existing local-library rescan.

**Tech Stack:** Rust 2024, Dioxus 0.7, Tokio blocking tasks, serde_json, reqwest URL parsing, yt-dlp, ffmpeg

## Global Constraints

- Keep the common implementation free of private-provider references so it can exist on both `public` and private `master`.
- Use `anvil tests` and `anvil check`; do not run equivalent Cargo or Dioxus builds locally.
- Accept only HTTPS YouTube and youtu.be URLs and pass the URL after `--` to avoid option injection.
- Download one video at a time as MP3 with embedded metadata and thumbnail.
- Preserve the existing configured `library_root`; fail clearly when it is unset.

---

### Task 1: YouTube import hook

**Files:**
- Create: `hooks/src/use_youtube.rs`
- Modify: `hooks/src/lib.rs`

**Interfaces:**
- Consumes: `UseLocalLibrary::rescan()`, `AppConfig::library_root`
- Produces: `YouTubePreview`, `UseYouTube`, `use_youtube()`, `UseYouTube::preview(String)`, `UseYouTube::download(UseLocalLibrary, Option<PathBuf>)`

- [x] **Step 1: Write the failing URL-validation test**

```rust
#[cfg(test)]
mod tests {
    use super::youtube_url;

    #[test]
    fn accepts_only_https_youtube_urls() {
        assert!(youtube_url("https://youtu.be/dQw4w9WgXcQ").is_ok());
        assert!(youtube_url("https://music.youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
        assert!(youtube_url("http://youtube.com/watch?v=x").is_err());
        assert!(youtube_url("https://youtube.com.evil.test/watch?v=x").is_err());
    }
}
```

- [x] **Step 2: Run the focused test and verify RED**

Run: `anvil tests`

Expected: FAIL because `youtube_url` does not exist.

- [x] **Step 3: Implement the minimum hook**

```rust
fn youtube_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "Paste a valid YouTube URL.".to_string())?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !matches!(host, "youtube.com" | "www.youtube.com" | "music.youtube.com" | "youtu.be")
    {
        return Err("Paste an HTTPS youtube.com or youtu.be URL.".into());
    }
    Ok(url)
}
```

Add one root context with `preview`, `status`, and `busy` signals. Its exact process arguments are:

```rust
const PREVIEW_ARGS: &[&str] = &[
    "--dump-single-json",
    "--no-playlist",
    "--skip-download",
    "--no-warnings",
    "--",
];

const DOWNLOAD_ARGS: &[&str] = &[
    "--no-playlist",
    "--extract-audio",
    "--audio-format",
    "mp3",
    "--audio-quality",
    "0",
    "--embed-metadata",
    "--embed-thumbnail",
    "--convert-thumbnails",
    "jpg",
    "--no-progress",
];
```

Append the validated URL to `PREVIEW_ARGS`. Append `--paths`, `<library_root>/YouTube`, `--output`, `%(uploader).40B - %(title).160B [%(id)s].%(ext)s`, `--print`, `after_move:filepath`, `--`, and the validated preview URL to `DOWNLOAD_ARGS`. Execute both commands inside `tokio::task::spawn_blocking`, surface the last stderr line on failure, and rescan after success.

- [x] **Step 4: Run the focused test and verify GREEN**

Run: `anvil tests`

Expected: PASS, including `accepts_only_https_youtube_urls`.

- [x] **Step 5: Commit**

```bash
git add hooks/src/lib.rs hooks/src/use_youtube.rs
git commit -m "Add YouTube library import backend"
```

### Task 2: Library preview and download surface

**Files:**
- Modify: `pages/src/library.rs`
- Modify: `nira/assets/css/library.css`
- Modify: `shell.nix`
- Modify: `README.md`

**Interfaces:**
- Consumes: `use_youtube()`, `use_config()`, `use_local_library()`
- Produces: `YouTubeImport` Dioxus component under Library → Local

- [x] **Step 1: Render the import component**

```rust
#[component]
fn YouTubeImport() -> Element {
    let youtube = use_youtube();
    let local = use_local_library();
    let config = use_config();
    let mut url = use_signal(String::new);
    let busy = *youtube.busy.read();

    rsx! {
        section { class: "yt-import",
            div { class: "yt-import-copy",
                span { class: "yt-import-kicker", "YouTube import" }
                h2 { "Bring a song into Nira" }
                p { "Preview one YouTube link, then save it locally as MP3." }
            }
            div { class: "yt-import-form",
                input {
                    class: "yt-import-input",
                    value: "{url.read()}",
                    placeholder: "https://youtube.com/watch?v=…",
                    oninput: move |e| url.set(e.value()),
                }
                button {
                    class: "sq-btn sq-btn-primary",
                    disabled: busy || url.read().trim().is_empty(),
                    onclick: move |_| youtube.preview(url.read().clone()),
                    "Preview"
                }
            }
        }
    }
}
```

The same component reads `youtube.preview`, `youtube.status`, and `youtube.busy`. When preview data exists, render its real thumbnail (or a music fallback), title, uploader, optional `hooks::fmt_time(duration)`, and this exact action:

```rust
Button {
    label: if busy { "Working…".to_string() } else { "Download MP3".to_string() },
    icon: Some(if busy {
        "fa-solid fa-circle-notch fa-spin".to_string()
    } else {
        "fa-solid fa-download".to_string()
    }),
    disabled: busy,
    on_click: move |_| youtube.download(local, config.peek().library_root.clone()),
}
```

- [x] **Step 2: Style with existing Nira tokens**

Use `var(--raise1)`, `var(--raise2)`, `var(--text)`, `var(--sub)`, `var(--faint)`, `var(--r)`, and `var(--rs)`. Do not add borders, shadows, gradients, hue accents, new fonts, or custom motion.

- [x] **Step 3: Add runtime tools**

```nix
packages = with pkgs; [
  # existing packages
  yt-dlp
];
```

The Nixpkgs `yt-dlp` wrapper supplies `ffmpeg-headless`; document `yt-dlp` and `ffmpeg` as runtime requirements for users launching a standalone build outside `shell.nix`.

- [x] **Step 4: Verify behavior and layout**

Run: `anvil tests`

Expected: all workspace tests pass.

Run: `anvil check`

Expected: workspace check passes.

Render Library → Local in both dark and light themes, inspect the real screenshots, and confirm the input, preview, status, and download action remain readable at narrow width.

- [x] **Step 5: Commit**

```bash
git add pages/src/library.rs nira/assets/css/library.css shell.nix README.md
git commit -m "Add YouTube import to Library"
```

### Task 3: Apply the public change to private master

**Files:**
- Modify: the same feature files on `master`

**Interfaces:**
- Consumes: the public feature commit
- Produces: equivalent feature behavior on `master` without importing private history into `public`

- [ ] **Step 1: Verify the public diff is free of private-provider references**

Run: `git show --stat --oneline public && git diff --check public^ public`

Expected: one clean feature commit with no whitespace errors.

- [ ] **Step 2: Apply the public commit to master**

```bash
feature_commit=$(git rev-parse public)
git switch master
git cherry-pick "$feature_commit"
```

Resolve only the existing `hooks/src/lib.rs` context installation overlap, retaining both private downloads and the new YouTube context.

- [ ] **Step 3: Verify master**

Run: `anvil tests`

Expected: all workspace tests pass.

Run: `anvil check`

Expected: workspace check passes.

- [ ] **Step 4: Inspect final state**

Run: `git diff --check && git status --short --branch`

Expected: no whitespace errors and a clean `master`; `public` and `master` both contain the feature.
