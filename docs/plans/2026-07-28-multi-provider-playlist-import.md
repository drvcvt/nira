# Multi-Provider Playlist Import Implementation Plan

> **For agentic workers:** Implement this plan task-by-task — one task per commit, run each task's test before moving on. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the one-click Spotify-only playlist import with a two-step Library importer that supports selective Spotify imports, public SoundCloud profile/playlist links, and link-only YouTube playlist downloads.

**Architecture:** Keep Nira's persisted `Playlist` format and provider playback model unchanged. Provider crates expose lightweight playlist catalogs first, the Library dialog owns provider/selection UI state, and only the selected source playlists are hydrated and written through one provider-neutral `UsePlaylists` import path. Spotify and SoundCloud tracks remain streamable provider tracks; YouTube playlist entries are downloaded with the existing `yt-dlp`/local-library path before the resulting local tracks are stored in a Nira playlist.

**Tech Stack:** Rust 2024, Dioxus 0.7, Tokio, reqwest/serde, existing Spotify and SoundCloud provider crates, existing `yt-dlp` + `ffmpeg`, existing JSON playlist store

## Global Constraints

- Implement on the current `public` branch without introducing names or code from private providers.
- Preserve the existing `playlists.json` schema; imported playlists still persist as normal Nira `Playlist` values.
- Never overwrite an already imported playlist or later local edits to it.
- Render imported playlists through the existing `PlaylistsPane` detail path,
  so their tracks keep the normal context menu and can be added to any other
  Nira playlist without provider-specific code.
- The first dialog step chooses Spotify, SoundCloud, or YouTube; the second dialog step shows importable playlists with per-row checkboxes plus `Select all` and `Deselect all`.
- Spotify loads the connected user's owned and collaborative playlist summaries first and fetches track pages only for selected playlists.
- SoundCloud "My playlists" means public playlists from one saved profile URL. Also accept public profile or playlist permalinks belonging to somebody else. Private SoundCloud playlists and account OAuth are out of scope for this iteration.
- YouTube accepts one HTTPS YouTube playlist link at a time. Import downloads playable entries as tagged MP3 files, rescans the local library, and creates one Nira playlist in source order.
- Reject non-HTTPS and non-provider URLs before sending them to a provider or process. Always place `--` before a user-supplied URL passed to `yt-dlp`.
- Continue past unavailable/non-track items where the provider has an explicit partial-success signal and report the skipped count.
- Failure policy is provider-specific and deliberate: Spotify's existing playlist-items 403 path skips that playlist; SoundCloud filters non-playable tracks but aborts catalog/hydration on request or payload errors; `yt-dlp --ignore-errors` skips unavailable YouTube entries. Authentication, rate-limit, malformed-payload, and general network failures abort the current provider batch so Nira never labels an unknown partial result as complete.
- Add no Rust dependencies. Add `ffmpeg` beside `yt-dlp` in `shell.nix` so
  audio extraction has an explicit runtime instead of depending on package
  wrapping details.
- Use only `anvil tests`, `anvil check`, and `anvil dev` for resource-heavy work.
- Use existing grayscale theme tokens, borderless brightness depth, `var(--r)`/`var(--rs)`, real checkboxes, dialog semantics, keyboard escape, live status text, reduced motion, and both themes.

## File Map

- Modify `hooks/src/use_playlists.rs`: generalize the existing Spotify-only merge into a source-scoped import API.
- Modify `hooks/src/lib.rs`: re-export the import DTOs and provider catalog DTOs consumed by `pages`.
- Modify `provider-spotify/src/lib.rs`: split playlist summary discovery from selected playlist hydration.
- Modify `provider-soundcloud/src/lib.rs`: validate/resolve public SoundCloud URLs, paginate public playlist catalogs, and hydrate selected playlists.
- Modify `config/src/lib.rs`: persist an optional public SoundCloud profile URL.
- Modify `pages/src/settings/connections.rs`: let the user save or clear that SoundCloud profile URL.
- Modify `hooks/src/use_youtube.rs`: inspect one YouTube playlist link and download/import it in source order.
- Modify `shell.nix`: expose `ffmpeg` to every named Anvil task.
- Create `pages/src/library_import.rs`: own the provider chooser, source-link step, playlist checkboxes, and result rendering.
- Modify `pages/src/lib.rs`: register the new Library-only import module.
- Modify `pages/src/library.rs`: replace the direct Spotify button/logic with the generic importer trigger.
- Modify `nira/assets/css/library.css`: style the dialog, provider choices, selection rows, and narrow layouts with existing tokens.

---

### Task 1: Provider-neutral persisted playlist imports

**Files:**
- Modify: `hooks/src/use_playlists.rs`
- Modify: `hooks/src/lib.rs`
- Test: `hooks/src/use_playlists.rs`

**Interfaces:**
- Consumes: existing `Playlist`, `Track`, and `UsePlaylists::mutate`
- Produces: `PlaylistImport`, `UsePlaylists::has_import(&str, &str)`, `UsePlaylists::import_external(&str, Vec<PlaylistImport>)`

- [ ] **Step 1: Replace the Spotify-only store test with source-scoped RED tests**

Delete `spotify_import_is_deduplicated_without_overwriting_local_edits` and replace it with these tests:

```rust
#[test]
fn external_imports_are_source_scoped_and_non_destructive() {
    let now = Utc::now();
    let mut items = vec![Playlist {
        id: "spotify-kept".into(),
        name: "My local rename".into(),
        tracks: Vec::new(),
        albums: Vec::new(),
        created_at: now,
        updated_at: now,
    }];

    let added = merge_external_playlists(
        &mut items,
        "spotify",
        vec![
            PlaylistImport {
                source_id: "kept".into(),
                name: "Remote rename".into(),
                tracks: Vec::new(),
            },
            PlaylistImport {
                source_id: "new".into(),
                name: "New playlist".into(),
                tracks: Vec::new(),
            },
        ],
    );

    assert_eq!(added, 1);
    assert_eq!(items[0].id, "spotify-new");
    assert_eq!(items[1].name, "My local rename");
}

#[test]
fn equal_provider_ids_do_not_collide_across_sources() {
    let mut items = Vec::new();
    let spotify = PlaylistImport {
        source_id: "42".into(),
        name: "Spotify".into(),
        tracks: Vec::new(),
    };
    let soundcloud = PlaylistImport {
        source_id: "42".into(),
        name: "SoundCloud".into(),
        tracks: Vec::new(),
    };

    assert_eq!(
        merge_external_playlists(&mut items, "spotify", vec![spotify]),
        1
    );
    assert_eq!(
        merge_external_playlists(&mut items, "soundcloud", vec![soundcloud]),
        1
    );
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|playlist| playlist.id == "spotify-42"));
    assert!(items.iter().any(|playlist| playlist.id == "soundcloud-42"));
}

#[test]
fn removed_import_can_be_imported_again() {
    let mut items = Vec::new();
    let imported = PlaylistImport {
        source_id: "road-trip".into(),
        name: "Road trip".into(),
        tracks: Vec::new(),
    };

    assert_eq!(
        merge_external_playlists(&mut items, "spotify", vec![imported.clone()]),
        1
    );
    items.retain(|playlist| playlist.id != "spotify-road-trip");
    assert_eq!(
        merge_external_playlists(&mut items, "spotify", vec![imported]),
        1
    );
}
```

- [ ] **Step 2: Run the workspace tests and verify RED**

Run: `anvil tests`

Expected: FAIL because `PlaylistImport` and `merge_external_playlists` do not exist.

- [ ] **Step 3: Add the minimum provider-neutral import API**

Add the DTO after `Playlist`:

```rust
#[derive(Debug, Clone)]
pub struct PlaylistImport {
    pub source_id: String,
    pub name: String,
    pub tracks: Vec<Track>,
}
```

Replace `UsePlaylists::import_spotify` with:

```rust
/// True when a source playlist has already been imported. Source ids are
/// namespaced so equal Spotify, SoundCloud, and YouTube ids cannot collide.
pub fn has_import(&self, source: &str, source_id: &str) -> bool {
    let id = external_playlist_id(source, source_id);
    self.items.read().iter().any(|playlist| playlist.id == id)
}

/// Import source playlists once without overwriting later local edits.
/// Returns the number of newly added playlists.
pub fn import_external(&self, source: &str, playlists: Vec<PlaylistImport>) -> usize {
    let mut added = 0;
    self.mutate(|items| {
        added = merge_external_playlists(items, source, playlists);
    });
    added
}

/// Compatibility for the current Library button. Task 6 removes this after
/// migrating the only caller to the provider-neutral dialog.
pub fn import_spotify(&self, playlists: Vec<(String, String, Vec<Track>)>) -> usize {
    self.import_external(
        "spotify",
        playlists
            .into_iter()
            .map(|(source_id, name, tracks)| PlaylistImport {
                source_id,
                name,
                tracks,
            })
            .collect(),
    )
}
```

Replace `merge_spotify_playlists` with:

```rust
fn external_playlist_id(source: &str, source_id: &str) -> String {
    format!("{}-{}", source.trim().to_ascii_lowercase(), source_id.trim())
}

fn merge_external_playlists(
    items: &mut Vec<Playlist>,
    source: &str,
    playlists: Vec<PlaylistImport>,
) -> usize {
    let now = Utc::now();
    let mut additions = Vec::new();
    for playlist in playlists {
        let id = external_playlist_id(source, &playlist.source_id);
        if items.iter().chain(&additions).any(|existing| existing.id == id) {
            continue;
        }
        additions.push(Playlist {
            id,
            name: playlist.name,
            tracks: playlist.tracks,
            albums: Vec::new(),
            created_at: now,
            updated_at: now,
        });
    }
    let added = additions.len();
    additions.append(items);
    *items = additions;
    added
}
```

Update the re-export in `hooks/src/lib.rs`:

```rust
pub use use_playlists::{
    Playlist, PlaylistAlbum, PlaylistImport, UsePlaylists, use_playlists,
};
```

- [ ] **Step 4: Run the tests and verify GREEN**

Run: `anvil tests`

Expected: PASS, including both source-scoping tests.

- [ ] **Step 5: Commit**

```bash
git add hooks/src/use_playlists.rs hooks/src/lib.rs
git commit -m "Generalize external playlist imports"
```

### Task 2: Two-phase Spotify playlist loading

**Files:**
- Modify: `provider-spotify/src/lib.rs`
- Modify: `hooks/src/lib.rs`
- Modify: `pages/src/library.rs`
- Test: `provider-spotify/src/lib.rs`

**Interfaces:**
- Consumes: existing Spotify OAuth/session, `/me`, `/me/playlists`, and `playlist_tracks`
- Produces: `SpotifyPlaylistSummary`, `SpotifyPlaylistCatalog`, `SpotifyProvider::playlist_catalog_for_import()`, `SpotifyProvider::playlists_for_import(Vec<SpotifyPlaylistSummary>)`

- [ ] **Step 1: Write a failing summary conversion test**

```rust
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
                "images": [],
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
    let page: SpPlaylistsPage = serde_json::from_str(json).expect("playlist page parses");
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
```

- [ ] **Step 2: Run tests and verify RED**

Run: `anvil tests`

Expected: FAIL because the catalog types and `playlist_summaries` do not exist.

- [ ] **Step 3: Add lightweight public catalog types**

Replace the public Spotify playlist import types with:

```rust
#[derive(Debug, Clone, PartialEq)]
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
```

Extend the simplified playlist API shapes:

```rust
#[derive(Deserialize)]
struct SpPlaylistBrief {
    id: String,
    name: String,
    #[serde(default)]
    collaborative: bool,
    owner: SpPlaylistOwner,
    #[serde(default)]
    images: Vec<SpImage>,
    #[serde(default)]
    items: Option<SpPlaylistItemsRef>,
    #[serde(default)]
    tracks: Option<SpPlaylistItemsRef>,
}

#[derive(Deserialize)]
struct SpPlaylistItemsRef {
    #[serde(default)]
    total: usize,
}
```

Add the pure conversion helper:

```rust
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
            cover_url: playlist.images.into_iter().next().map(|image| image.url),
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
```

- [ ] **Step 4: Split catalog discovery from selected hydration**

Replace the current zero-argument `playlists_for_import` method with:

```rust
/// List importable playlists without loading their track pages.
pub async fn playlist_catalog_for_import(&self) -> ProviderResult<SpotifyPlaylistCatalog> {
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
```

In the existing `playlist_tracks`, replace the duplicated terminal check and
`offset += received` with the same helper:

```rust
match next_playlist_offset(offset, received, page.next.is_some()) {
    Some(next) => offset = next,
    None => break,
}
```

Do not catch `AuthRequired`, `RateLimited`, `Malformed`, or `Network` inside
the selected-playlist loop. Only the existing `fetch_json_allow_forbidden`
`Ok(None)` result is a playlist-level skip; every other error aborts the batch.

Re-export the new types from `hooks/src/lib.rs` without making `pages` depend directly on the provider crate:

```rust
pub use provider_spotify::{
    SpotifyPlaylist, SpotifyPlaylistCatalog, SpotifyPlaylistImport,
    SpotifyPlaylistSummary, SpotifyProvider,
};
```

Change the existing private import at the top of `hooks/src/lib.rs` to avoid importing `SpotifyProvider` twice.

- [ ] **Step 5: Keep the existing direct caller compiling until the dialog lands**

In the current Spotify button handler in `pages/src/library.rs`, replace:

```rust
match spotify.playlists_for_import().await {
```

with a two-call result that still preserves the current import-all behavior:

```rust
let result = match spotify.playlist_catalog_for_import().await {
    Ok(catalog) => {
        let skipped_catalog = catalog.skipped_playlists;
        spotify
            .playlists_for_import(catalog.playlists)
            .await
            .map(|mut result| {
                result.skipped_playlists += skipped_catalog;
                result
            })
    }
    Err(error) => Err(error),
};
match result {
```

Task 6 deletes this temporary direct handler entirely. Keeping it functional here makes every task commit independently testable.

- [ ] **Step 6: Run the tests and compile check**

Run: `anvil tests`

Expected: PASS, including catalog conversion and unsupported-item filtering.

Run: `anvil check`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add provider-spotify/src/lib.rs hooks/src/lib.rs pages/src/library.rs
git commit -m "Split Spotify playlist discovery from import"
```

### Task 3: Public SoundCloud playlist catalogs and track hydration

**Files:**
- Modify: `provider-soundcloud/src/lib.rs`
- Modify: `hooks/src/lib.rs`
- Test: `provider-soundcloud/src/lib.rs`

**Interfaces:**
- Consumes: existing rotating public web `client_id`, `fetch_json`, `with_client_id`, `ScTrack`, and `sc_to_track`
- Produces: `validate_soundcloud_url(&str)`, `SoundCloudPlaylistSummary`, `SoundCloudPlaylistCatalog`, `SoundCloudPlaylistImport`, `SoundCloudProvider::playlist_catalog_from_url(&str)`, `SoundCloudProvider::playlists_for_import(Vec<SoundCloudPlaylistSummary>)`

- [ ] **Step 1: Write failing URL and payload tests**

```rust
#[test]
fn accepts_only_https_soundcloud_urls() {
    assert!(validate_soundcloud_url("https://soundcloud.com/ninja-tune").is_ok());
    assert!(validate_soundcloud_url("https://on.soundcloud.com/abc123").is_ok());
    assert!(validate_soundcloud_url("http://soundcloud.com/ninja-tune").is_err());
    assert!(validate_soundcloud_url("https://soundcloud.com.evil.test/x").is_err());
    assert!(validate_soundcloud_url("https://soundcloud.com:444/ninja-tune").is_err());
}

#[test]
fn public_playlist_page_keeps_summary_metadata() {
    let json = r#"{
        "collection": [{
            "id": 42,
            "title": "Night drive",
            "artwork_url": "https://i1.sndcdn.com/artworks-x-large.jpg",
            "track_count": 3
        }],
        "next_href": null
    }"#;
    let page: ScPage<ScPlaylistBrief> =
        serde_json::from_str(json).expect("playlist page parses");
    let summary = soundcloud_summary(page.collection.into_iter().next().unwrap());

    assert_eq!(summary.id, 42);
    assert_eq!(summary.title, "Night drive");
    assert_eq!(summary.track_count, 3);
    assert!(summary.cover_url.unwrap().contains("-t500x500."));
}

#[test]
fn next_page_must_remain_on_soundcloud_api() {
    assert!(soundcloud_api_url(
        "https://api-v2.soundcloud.com/users/1/playlists?cursor=next",
        "client"
    )
    .is_ok());
    assert!(soundcloud_api_url("https://evil.test/steal", "client").is_err());
    assert!(soundcloud_api_url(
        "https://api-v2.soundcloud.com:444/users/1/playlists",
        "client"
    )
    .is_err());
    assert_eq!(
        soundcloud_api_url(
            "https://api-v2.soundcloud.com/users/1/playlists?cursor=next&client_id=stale",
            "fresh"
        )
        .unwrap(),
        "https://api-v2.soundcloud.com/users/1/playlists?cursor=next&client_id=fresh"
    );
}

#[test]
fn cursor_page_deserializes_and_keeps_next_href() {
    let json = r#"{
        "collection": [{
            "id": 42,
            "title": "Page one",
            "track_count": 1
        }],
        "next_href": "https://api-v2.soundcloud.com/users/1/playlists?cursor=next"
    }"#;
    let page: ScPage<ScPlaylistBrief> = serde_json::from_str(json).unwrap();

    assert_eq!(page.collection.len(), 1);
    assert_eq!(
        soundcloud_api_url(page.next_href.as_deref().unwrap(), "client").unwrap(),
        "https://api-v2.soundcloud.com/users/1/playlists?cursor=next&client_id=client"
    );
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `anvil tests`

Expected: FAIL because the URL validator, page shapes, and summary converter do not exist.

- [ ] **Step 3: Add URL validation and public DTOs**

Add these public types near `SoundCloudProvider`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct SoundCloudPlaylistSummary {
    pub id: u64,
    pub title: String,
    pub cover_url: Option<String>,
    pub track_count: usize,
}

#[derive(Debug, Clone)]
pub struct SoundCloudPlaylist {
    pub id: u64,
    pub title: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct SoundCloudPlaylistCatalog {
    pub playlists: Vec<SoundCloudPlaylistSummary>,
}

#[derive(Debug, Clone)]
pub struct SoundCloudPlaylistImport {
    pub playlists: Vec<SoundCloudPlaylist>,
    pub skipped_items: usize,
}
```

Add strict URL helpers. The fixed host check prevents arbitrary URLs and untrusted `next_href` values from turning the provider into an HTTP client for other hosts:

```rust
pub fn validate_soundcloud_url(raw: &str) -> ProviderResult<()> {
    let url = url::Url::parse(raw.trim())
        .map_err(|_| ProviderError::Other("Paste a valid SoundCloud URL.".into()))?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !matches!(
            host,
            "soundcloud.com" | "www.soundcloud.com" | "on.soundcloud.com"
        )
    {
        return Err(ProviderError::Other(
            "Paste an HTTPS soundcloud.com URL.".into(),
        ));
    }
    Ok(())
}

fn soundcloud_api_url(raw: &str, client_id: &str) -> ProviderResult<String> {
    let mut url = url::Url::parse(raw)
        .map_err(|_| ProviderError::Malformed("invalid SoundCloud API URL".into()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("api-v2.soundcloud.com")
        || url.port_or_known_default() != Some(443)
    {
        return Err(ProviderError::Malformed(
            "SoundCloud pagination left the API host".into(),
        ));
    }
    let query: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "client_id")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(&key, &value);
        }
        pairs.append_pair("client_id", client_id);
    }
    Ok(url.to_string())
}
```

Add the minimal response shapes:

```rust
#[derive(Deserialize)]
struct ScResolved {
    kind: String,
    id: u64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    artwork_url: Option<String>,
    #[serde(default)]
    track_count: usize,
}

#[derive(Deserialize)]
struct ScPlaylistBrief {
    id: u64,
    title: String,
    #[serde(default)]
    artwork_url: Option<String>,
    #[serde(default)]
    track_count: usize,
}

#[derive(Deserialize)]
struct ScPage<T> {
    #[serde(default)]
    collection: Vec<T>,
    #[serde(default)]
    next_href: Option<String>,
}

fn soundcloud_summary(raw: ScPlaylistBrief) -> SoundCloudPlaylistSummary {
    SoundCloudPlaylistSummary {
        id: raw.id,
        title: raw.title,
        cover_url: raw.artwork_url.map(upgrade_artwork),
        track_count: raw.track_count,
    }
}
```

- [ ] **Step 4: Resolve a profile or playlist URL into a catalog**

Add these methods in the existing inherent `impl SoundCloudProvider`:

```rust
/// Resolve a public SoundCloud profile or playlist permalink.
pub async fn playlist_catalog_from_url(
    &self,
    raw_url: &str,
) -> ProviderResult<SoundCloudPlaylistCatalog> {
    validate_soundcloud_url(raw_url)?;
    let raw_url = raw_url.trim().to_string();
    self.with_client_id(|client_id| {
        let raw_url = raw_url.clone();
        async move {
            let mut resolve = url::Url::parse(&format!("{SC_API}/resolve"))
                .map_err(|error| ProviderError::Other(error.to_string()))?;
            resolve
                .query_pairs_mut()
                .append_pair("url", &raw_url)
                .append_pair("client_id", &client_id);
            let resolved: ScResolved = self.fetch_json(resolve.as_str()).await?;
            match resolved.kind.as_str() {
                "user" => self.user_playlist_catalog(resolved.id, &client_id).await,
                "playlist" | "system-playlist" => {
                    let title = resolved.title.ok_or_else(|| {
                        ProviderError::Malformed("SoundCloud playlist has no title".into())
                    })?;
                    Ok(SoundCloudPlaylistCatalog {
                        playlists: vec![SoundCloudPlaylistSummary {
                            id: resolved.id,
                            title,
                            cover_url: resolved.artwork_url.map(upgrade_artwork),
                            track_count: resolved.track_count,
                        }],
                    })
                }
                _ => Err(ProviderError::Other(
                    "Paste a SoundCloud profile or playlist URL.".into(),
                )),
            }
        }
    })
    .await
}

async fn user_playlist_catalog(
    &self,
    user_id: u64,
    client_id: &str,
) -> ProviderResult<SoundCloudPlaylistCatalog> {
    let first = format!(
        "{SC_API}/users/{user_id}/playlists?representation=compact\
         &linked_partitioning=true&limit=200"
    );
    let mut next = Some(soundcloud_api_url(&first, client_id)?);
    let mut playlists = Vec::new();
    while let Some(url) = next.take() {
        let page: ScPage<ScPlaylistBrief> = self.fetch_json(&url).await?;
        playlists.extend(page.collection.into_iter().map(soundcloud_summary));
        next = page
            .next_href
            .as_deref()
            .map(|url| soundcloud_api_url(url, client_id))
            .transpose()?;
    }
    Ok(SoundCloudPlaylistCatalog { playlists })
}
```

- [ ] **Step 5: Hydrate only selected SoundCloud playlists**

Add:

```rust
pub async fn playlists_for_import(
    &self,
    selected: Vec<SoundCloudPlaylistSummary>,
) -> ProviderResult<SoundCloudPlaylistImport> {
    self.with_client_id(|client_id| {
        let selected = selected.clone();
        async move {
            let mut result = SoundCloudPlaylistImport {
                playlists: Vec::new(),
                skipped_items: 0,
            };
            for playlist in selected {
                let first = format!(
                    "{SC_API}/playlists/{}/tracks?access=playable\
                     &linked_partitioning=true&limit=200",
                    playlist.id
                );
                let mut next = Some(soundcloud_api_url(&first, &client_id)?);
                let mut tracks = Vec::new();
                while let Some(url) = next.take() {
                    let page: ScPage<ScTrack> = self.fetch_json(&url).await?;
                    tracks.extend(page.collection.into_iter().map(sc_to_track));
                    next = page
                        .next_href
                        .as_deref()
                        .map(|url| soundcloud_api_url(url, &client_id))
                        .transpose()?;
                }
                result.skipped_items += playlist.track_count.saturating_sub(tracks.len());
                result.playlists.push(SoundCloudPlaylist {
                    id: playlist.id,
                    title: playlist.title,
                    tracks,
                });
            }
            Ok(result)
        }
    })
    .await
}
```

Keep request failures fatal in this loop. SoundCloud uses 401/403 to signal a
rotated public web `client_id`, so `with_client_id` must retain ownership of
that retry path instead of treating 403 as a missing playlist. The explicit
partial-success signal here is `access=playable`: the difference between
`track_count` and returned tracks becomes `skipped_items`.

Re-export the concrete provider, DTOs, and validator from `hooks/src/lib.rs`:

```rust
pub use provider_soundcloud::{
    SoundCloudPlaylist, SoundCloudPlaylistCatalog, SoundCloudPlaylistImport,
    SoundCloudPlaylistSummary, SoundCloudProvider, validate_soundcloud_url,
};
```

Remove the duplicate private import of `SoundCloudProvider`.

- [ ] **Step 6: Run tests and check**

Run: `anvil tests`

Expected: PASS, including URL, payload, pagination-host, and all existing SoundCloud playback tests.

Run: `anvil check`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add provider-soundcloud/src/lib.rs hooks/src/lib.rs
git commit -m "Add public SoundCloud playlist import"
```

### Task 4: Saved SoundCloud profile URL

**Files:**
- Modify: `config/src/lib.rs`
- Modify: `pages/src/settings/connections.rs`
- Test: `config/src/lib.rs`

**Interfaces:**
- Consumes: existing `AppConfig` persistence and SoundCloud settings card
- Produces: `AppConfig::soundcloud_profile_url: Option<String>`

- [ ] **Step 1: Write the backward-compatibility test**

```rust
#[test]
fn missing_soundcloud_profile_url_defaults_to_none() {
    let cfg: AppConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.soundcloud_profile_url, None);
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `anvil tests`

Expected: FAIL because `AppConfig` has no `soundcloud_profile_url`.

- [ ] **Step 3: Add the optional config field**

Add beside `spotify_client_id`:

```rust
/// Public SoundCloud profile used by Library → Import → SoundCloud.
/// This is a permalink, not a credential; private playlists need OAuth and
/// intentionally remain out of scope.
#[serde(default)]
pub soundcloud_profile_url: Option<String>,
```

Add `soundcloud_profile_url: None` to `AppConfig::default`.

- [ ] **Step 4: Add the SoundCloud profile control**

Initialize one draft near the existing SoundCloud signals:

```rust
let initial_sc_profile = config
    .read()
    .soundcloud_profile_url
    .clone()
    .unwrap_or_default();
let mut sc_profile_draft = use_signal(move || initial_sc_profile);
```

Insert this row at the start of the existing SoundCloud `SettingsCard`, before the client-id refresh action:

```rust
div { class: "settings-row",
    label { r#for: "sc-profile-url", "Public profile URL" }
    input {
        id: "sc-profile-url",
        r#type: "url",
        class: "settings-input",
        placeholder: "https://soundcloud.com/your-name",
        value: "{sc_profile_draft.read()}",
        oninput: move |event| sc_profile_draft.set(event.value()),
    }
    Button {
        label: "Save".to_string(),
        variant: ButtonVariant::Ghost,
        size: ButtonSize::Sm,
        on_click: move |_| {
            let value = sc_profile_draft.read().trim().to_string();
            if !value.is_empty()
                && let Err(error) = hooks::validate_soundcloud_url(&value)
            {
                sc_status.set(Some(error.to_string()));
                return;
            }
            let save = {
                let mut current = config.write();
                current.soundcloud_profile_url =
                    (!value.is_empty()).then_some(value);
                current.save()
            };
            sc_status.set(Some(match save {
                Ok(()) => "SoundCloud profile saved.".into(),
                Err(error) => format!("Save failed: {error}"),
            }));
        },
    }
}
```

Change the SoundCloud explanatory copy to exactly:

```text
Save a public profile for one-click playlist discovery. Public profile and playlist links need no SoundCloud login.
```

- [ ] **Step 5: Run tests and check**

Run: `anvil tests`

Expected: PASS, including old config deserialization.

Run: `anvil check`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add config/src/lib.rs pages/src/settings/connections.rs
git commit -m "Remember a public SoundCloud profile"
```

### Task 5: YouTube playlist inspection and background import

**Files:**
- Modify: `hooks/src/use_youtube.rs`
- Modify: `hooks/src/lib.rs`
- Modify: `shell.nix`
- Test: `hooks/src/use_youtube.rs`

**Interfaces:**
- Consumes: existing `run_ytdlp`, `youtube_url`, `UseLocalLibrary`, `UseDownloads`, `UsePlaylists`, `provider_local::scan`, and `provider_local::track_uri`
- Produces: `YouTubePlaylist`, `UseYouTube::inspect_playlist(String)`, `UseYouTube::import_playlist(...)`

- [ ] **Step 1: Make the existing audio runtime explicit**

Add `ffmpeg` beside `yt-dlp` in `shell.nix`:

```nix
    ffmpeg
    yt-dlp
```

Do not embed a `nix-shell -p` package list in `anvil.toml`; every named task
already enters this shared shell.

- [ ] **Step 2: Write failing playlist parsing and source-order tests**

```rust
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
        ordered.iter().map(|track| track.title.as_str()).collect::<Vec<_>>(),
        vec!["Second", "First"]
    );
}
```

Also extend the existing `accepts_only_https_youtube_urls` test:

```rust
assert!(youtube_url("https://youtube.com:444/playlist?list=PLx").is_err());
```

Add a test-only `local_test_track` beside the tests using the existing `Track` fields and `provider_local::track_uri(path)`. It must create `ProviderId::Local` tracks with empty artists, no album/cover/MBID, zero duration, and the supplied title.

```rust
fn local_test_track(path: &std::path::Path, title: &str) -> Track {
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
```

- [ ] **Step 3: Run tests and verify RED**

Run: `anvil tests`

Expected: FAIL because `YouTubePlaylist`, `parse_playlist`, and `order_downloaded_tracks` do not exist.

- [ ] **Step 4: Add the playlist DTO and parser**

Before the new DTO, tighten the existing `youtube_url` condition so every
single-video and playlist path rejects non-default ports:

```rust
if url.scheme() != "https" || url.port_or_known_default() != Some(443) || !allowed {
    return Err("Paste an HTTPS youtube.com or youtu.be URL.".into());
}
```

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct YouTubePlaylist {
    pub id: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub track_count: usize,
    pub url: String,
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
```

- [ ] **Step 5: Add link inspection**

```rust
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
```

The Library dialog owns the short-lived "Loading playlist…" state for this inspection. Do not overwrite the existing single-video `preview` signal.

- [ ] **Step 6: Add ordered background download/import**

Add:

```rust
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
```

Add this root-owned operation to `impl UseYouTube`:

```rust
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
        let result = async {
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
                let existing = if imported == 0 { " Already imported." } else { "" };
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
```

Replace the existing crate import at the top of `hooks/src/use_youtube.rs` with:

```rust
use crate::{
    PlaylistImport, Track, UseDownloads, UseLocalLibrary, UsePlaylists,
};
```

Inside the test module, import the exact additional types used by `local_test_track`:

```rust
use crate::{ProviderId, TrackUri};
```

Re-export `YouTubePlaylist` from `hooks/src/lib.rs`:

```rust
pub use use_youtube::{UseYouTube, YouTubePlaylist, YouTubePreview, use_youtube};
```

- [ ] **Step 7: Run tests and check**

Run: `anvil tests`

Expected: PASS, including URL validation, playlist parsing, order preservation, and existing single-video download messages.

Run: `anvil check`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add hooks/src/use_youtube.rs hooks/src/lib.rs shell.nix
git commit -m "Add YouTube playlist imports"
```

### Task 6: Provider chooser and selective playlist dialog

**Files:**
- Create: `pages/src/library_import.rs`
- Modify: `hooks/src/use_playlists.rs`
- Modify: `pages/src/lib.rs`
- Modify: `pages/src/library.rs`
- Modify: `nira/assets/css/library.css`
- Test: `pages/src/library_import.rs`

**Interfaces:**
- Consumes: `use_spotify`, `use_soundcloud`, `use_youtube`, `use_config`, `use_local_library`, `use_downloads`, `UsePlaylists`, and provider catalog DTOs
- Produces: `PlaylistImporter(open: Signal<bool>)`

- [ ] **Step 1: Register the module and write the failing selection helper test**

Add to `pages/src/lib.rs`:

```rust
mod library_import;
```

Start `pages/src/library_import.rs` with the state types and test. Deliberately leave `select_all` undefined for the RED run:

```rust
use components::SearchBar;
use dioxus::prelude::*;
use std::{path::PathBuf, sync::Arc};
use hooks::{
    PlaylistImport, SoundCloudPlaylistSummary, SoundCloudProvider,
    SpotifyPlaylistSummary, SpotifyProvider, UseDownloads, UseLocalLibrary,
    UsePlaylists, UseYouTube, YouTubePlaylist, use_config, use_downloads,
    use_local_library, use_playlists, use_soundcloud, use_spotify, use_youtube,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportProvider {
    Spotify,
    SoundCloud,
    YouTube,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportStep {
    Provider,
    Source,
    Select,
    Complete,
}

#[derive(Clone, PartialEq)]
struct ImportChoice {
    source_id: String,
    name: String,
    cover_url: Option<String>,
    track_count: usize,
    selected: bool,
    already_imported: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_all_never_reselects_existing_imports() {
        let mut choices = vec![
            ImportChoice {
                source_id: "new".into(),
                name: "New".into(),
                cover_url: None,
                track_count: 1,
                selected: false,
                already_imported: false,
            },
            ImportChoice {
                source_id: "old".into(),
                name: "Old".into(),
                cover_url: None,
                track_count: 2,
                selected: false,
                already_imported: true,
            },
        ];

        select_all(&mut choices, true);
        assert!(choices[0].selected);
        assert!(!choices[1].selected);

        select_all(&mut choices, false);
        assert!(choices.iter().all(|choice| !choice.selected));
    }

    #[test]
    fn import_status_omits_zero_counts() {
        assert_eq!(
            import_message(2, 0, 1, 3),
            "Imported 2 playlists. 1 playlist could not be read. 3 unavailable items were skipped."
        );
        assert_eq!(import_message(1, 0, 0, 0), "Imported 1 playlist.");
    }
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `anvil tests`

Expected: FAIL because `select_all` and `import_message` do not exist.

- [ ] **Step 3: Implement selection and replace the direct Spotify trigger**

Add the minimum helper above the test module:

```rust
fn select_all(choices: &mut [ImportChoice], selected: bool) {
    for choice in choices {
        choice.selected = selected && !choice.already_imported;
    }
}

fn import_message(
    added: usize,
    existing: usize,
    skipped_playlists: usize,
    skipped_items: usize,
) -> String {
    let mut parts = vec![format!(
        "Imported {added} {}.",
        if added == 1 { "playlist" } else { "playlists" }
    )];
    if existing > 0 {
        parts.push(format!("{existing} already imported."));
    }
    if skipped_playlists > 0 {
        parts.push(format!(
            "{skipped_playlists} {} could not be read.",
            if skipped_playlists == 1 {
                "playlist"
            } else {
                "playlists"
            }
        ));
    }
    if skipped_items > 0 {
        parts.push(format!(
            "{skipped_items} unavailable {} skipped.",
            if skipped_items == 1 { "item was" } else { "items were" }
        ));
    }
    parts.join(" ")
}
```

In `pages/src/library.rs`:

1. Remove `use_spotify` from the imports.
2. Remove `importing`, `import_status`, `spotify_connected`, `import_label`, `import_title`, and the complete direct Spotify `spawn` block.
3. Add `let mut importer_open = use_signal(|| false);` beside `new_name`.
4. Replace the old Spotify import button with:

```rust
button {
    class: "sq-btn sq-btn-ghost sq-sm",
    onclick: move |_| importer_open.set(true),
    i { class: "fa-solid fa-file-import" }
    " Import"
}
```

5. Render the always-mounted dialog immediately after `.lib-pl-create`:

```rust
crate::library_import::PlaylistImporter { open: importer_open }
```

6. Change the empty-state sentence to:

```text
Create one above, import from Spotify, SoundCloud, or YouTube, or right-click any track and pick "Add to playlist".
```

7. Delete the temporary `UsePlaylists::import_spotify` compatibility method
   from Task 1. Its only caller is now gone; keep only `import_external`.

- [ ] **Step 4: Add exact catalog-to-choice helpers**

Add these helpers above the component:

```rust
fn spotify_choices(
    playlists: UsePlaylists,
    catalog: hooks::SpotifyPlaylistCatalog,
) -> Vec<ImportChoice> {
    catalog
        .playlists
        .into_iter()
        .map(|playlist| {
            let already_imported = playlists.has_import("spotify", &playlist.id);
            ImportChoice {
                source_id: playlist.id,
                name: playlist.name,
                cover_url: playlist.cover_url,
                track_count: playlist.track_count,
                selected: !already_imported,
                already_imported,
            }
        })
        .collect()
}

fn soundcloud_choices(
    playlists: UsePlaylists,
    catalog: hooks::SoundCloudPlaylistCatalog,
) -> Vec<ImportChoice> {
    catalog
        .playlists
        .into_iter()
        .map(|playlist| {
            let source_id = playlist.id.to_string();
            let already_imported = playlists.has_import("soundcloud", &source_id);
            ImportChoice {
                source_id,
                name: playlist.title,
                cover_url: playlist.cover_url,
                track_count: playlist.track_count,
                selected: !already_imported,
                already_imported,
            }
        })
        .collect()
}

fn youtube_choices(
    playlists: UsePlaylists,
    playlist: &YouTubePlaylist,
) -> Vec<ImportChoice> {
    let already_imported = playlists.has_import("youtube", &playlist.id);
    vec![ImportChoice {
        source_id: playlist.id.clone(),
        name: playlist.title.clone(),
        cover_url: playlist.cover_url.clone(),
        track_count: playlist.track_count,
        selected: !already_imported,
        already_imported,
    }]
}
```

- [ ] **Step 5: Implement the dialog state and provider/source flow**

Add the async launch helpers first. They own every success/error transition, so
provider cards, Enter submission, and `Load` cannot drift into different
behavior:

```rust
fn load_spotify_catalog(
    spotify: Arc<SpotifyProvider>,
    playlists: UsePlaylists,
    mut choices: Signal<Vec<ImportChoice>>,
    mut step: Signal<ImportStep>,
    mut busy: Signal<bool>,
    mut status: Signal<Option<String>>,
    mut catalog_skipped: Signal<usize>,
) {
    busy.set(true);
    status.set(Some("Loading Spotify playlists…".into()));
    catalog_skipped.set(0);
    spawn(async move {
        match spotify.playlist_catalog_for_import().await {
            Ok(catalog) => {
                let skipped = catalog.skipped_playlists;
                choices.set(spotify_choices(playlists, catalog));
                catalog_skipped.set(skipped);
                if skipped > 0 {
                    status.set(Some(format!(
                        "{skipped} followed Spotify playlists cannot be imported."
                    )));
                } else {
                    status.set(None);
                }
                busy.set(false);
                step.set(ImportStep::Select);
            }
            Err(error) => {
                busy.set(false);
                status.set(Some(format!("Spotify: {error}")));
            }
        }
    });
}

fn load_soundcloud_catalog(
    raw_url: String,
    soundcloud: Arc<SoundCloudProvider>,
    playlists: UsePlaylists,
    mut choices: Signal<Vec<ImportChoice>>,
    mut step: Signal<ImportStep>,
    mut busy: Signal<bool>,
    mut status: Signal<Option<String>>,
    mut catalog_skipped: Signal<usize>,
) {
    busy.set(true);
    status.set(Some("Loading SoundCloud playlists…".into()));
    catalog_skipped.set(0);
    spawn(async move {
        match soundcloud.playlist_catalog_from_url(&raw_url).await {
            Ok(catalog) => {
                choices.set(soundcloud_choices(playlists, catalog));
                status.set(None);
                busy.set(false);
                step.set(ImportStep::Select);
            }
            Err(error) => {
                busy.set(false);
                step.set(ImportStep::Source);
                status.set(Some(format!("SoundCloud: {error}")));
            }
        }
    });
}

fn load_youtube_catalog(
    raw_url: String,
    youtube: UseYouTube,
    playlists: UsePlaylists,
    mut choices: Signal<Vec<ImportChoice>>,
    mut youtube_playlist: Signal<Option<YouTubePlaylist>>,
    mut step: Signal<ImportStep>,
    mut busy: Signal<bool>,
    mut status: Signal<Option<String>>,
    mut catalog_skipped: Signal<usize>,
) {
    busy.set(true);
    status.set(Some("Loading YouTube playlist…".into()));
    catalog_skipped.set(0);
    spawn(async move {
        match youtube.inspect_playlist(raw_url).await {
            Ok(playlist) => {
                choices.set(youtube_choices(playlists, &playlist));
                youtube_playlist.set(Some(playlist));
                status.set(None);
                busy.set(false);
                step.set(ImportStep::Select);
            }
            Err(error) => {
                busy.set(false);
                status.set(Some(format!("YouTube: {error}")));
            }
        }
    });
}

fn load_source_catalog(
    provider: Option<ImportProvider>,
    raw_url: String,
    soundcloud: Arc<SoundCloudProvider>,
    youtube: UseYouTube,
    playlists: UsePlaylists,
    choices: Signal<Vec<ImportChoice>>,
    youtube_playlist: Signal<Option<YouTubePlaylist>>,
    step: Signal<ImportStep>,
    mut busy: Signal<bool>,
    mut status: Signal<Option<String>>,
    catalog_skipped: Signal<usize>,
) {
    if raw_url.trim().is_empty() {
        status.set(Some("Paste a playlist link first.".into()));
        return;
    }
    match provider {
        Some(ImportProvider::SoundCloud) => load_soundcloud_catalog(
            raw_url,
            soundcloud,
            playlists,
            choices,
            step,
            busy,
            status,
            catalog_skipped,
        ),
        Some(ImportProvider::YouTube) => load_youtube_catalog(
            raw_url,
            youtube,
            playlists,
            choices,
            youtube_playlist,
            step,
            busy,
            status,
            catalog_skipped,
        ),
        _ => {
            busy.set(false);
            status.set(Some("Choose SoundCloud or YouTube first.".into()));
        }
    }
}
```

`PlaylistImporter` then owns these signals:

```rust
#[component]
pub(crate) fn PlaylistImporter(mut open: Signal<bool>) -> Element {
    let playlists = use_playlists();
    let spotify = use_spotify();
    let soundcloud = use_soundcloud();
    let youtube = use_youtube();
    let local = use_local_library();
    let downloads = use_downloads();
    let config = use_config();

    let mut step = use_signal(|| ImportStep::Provider);
    let mut provider = use_signal(|| None::<ImportProvider>);
    let mut choices = use_signal(Vec::<ImportChoice>::new);
    let mut source_url = use_signal(String::new);
    let mut youtube_playlist = use_signal(|| None::<YouTubePlaylist>);
    let mut busy = use_signal(|| false);
    let mut status = use_signal(|| None::<String>);
    let mut catalog_skipped = use_signal(|| 0usize);
    let mut was_open = use_signal(|| false);
```

Reset only on the closed-to-open transition. This effect subscribes only to
`open`; all other signals use `peek` so catalog and selection changes cannot
push duplicate entries onto the overlay focus stack:

```rust
use_effect(move || {
    let is_open = *open.read();
    let previously_open = *was_open.peek();
    if is_open == previously_open {
        return;
    }
    if is_open && !*busy.peek() {
        step.set(ImportStep::Provider);
        provider.set(None);
        choices.set(Vec::new());
        source_url.set(String::new());
        youtube_playlist.set(None);
        status.set(None);
        catalog_skipped.set(0);
    }
    components::overlay_focus(
        is_open,
        ".playlist-import.open button[data-import-provider]:not(:disabled)",
    );
    was_open.set(is_open);
});
```

Step changes mount new controls. Give the source `SearchBar` `autofocus: true`,
give the first non-disabled selection checkbox `autofocus: true`, and give the
`Done` button in `Complete` `autofocus: true`. This covers focus after every
transition without calling `overlay_focus` again or growing its focus stack.

Provider behavior is exact:

| Provider | Select action |
|---|---|
| Spotify | If disconnected, the card is disabled and says `Connect Spotify in Settings first.` Otherwise set `busy`, call `spotify.playlist_catalog_for_import().await`, convert with `spotify_choices`, show the followed/skipped count in `status`, and advance directly to `Select`. |
| SoundCloud | Set `provider`. If `config.soundcloud_profile_url` exists, copy it to `source_url`, load it automatically with `soundcloud.playlist_catalog_from_url`, convert with `soundcloud_choices`, and advance to `Select`. If no profile is saved, advance to `Source`. |
| YouTube | Set `provider` and advance to `Source`; no provider request runs before a link is submitted. |

The `Source` step renders one `SearchBar` and one `Load` button. Its copy and placeholder are:

| Provider | Heading | Copy | Placeholder |
|---|---|---|---|
| SoundCloud | `SoundCloud link` | `Paste a public profile or playlist link. A profile shows all of its public playlists.` | `https://soundcloud.com/artist-or-playlist` |
| YouTube | `YouTube playlist link` | `Paste one playlist link. Nira downloads its available entries as MP3 files.` | `https://youtube.com/playlist?list=…` |

Submitting SoundCloud calls `playlist_catalog_from_url`, stores `soundcloud_choices`, and advances to `Select`.

Submitting YouTube calls `youtube.inspect_playlist`, stores the returned `YouTubePlaylist`, stores `youtube_choices`, and advances to `Select`.

Every provider/link handler sets `busy` and a provider-specific loading status
before its async call. Each `Ok` and `Err` arm clears `busy` before changing
the step or final status.

On error, Spotify stays on `Provider`, YouTube stays on `Source`, and
SoundCloud goes to `Source`. That last transition is required when a saved
profile URL has become private, deleted, or invalid, so the user can paste a
different public link immediately. Keep the provider-prefixed message visible.

- [ ] **Step 6: Implement the selection and import actions**

The `Select` step renders a real checkbox per row. Address updates by
`source_id`, never by the visual index:

```rust
onchange: {
    let source_id = choice.source_id.clone();
    move |event: FormEvent| {
        let mut next = choices.peek().clone();
        if let Some(choice) = next
            .iter_mut()
            .find(|choice| choice.source_id == source_id)
        {
            choice.selected = event.checked() && !choice.already_imported;
        }
        choices.set(next);
    }
},
```

Add this complete import launcher. It hydrates only
selected rows, keeps catalog-level Spotify skips, persists through the one
generic store method, and leaves provider errors inside the open dialog:

```rust
fn import_selected(
    provider: Option<ImportProvider>,
    current: Vec<ImportChoice>,
    catalog_skipped: usize,
    spotify: Arc<SpotifyProvider>,
    soundcloud: Arc<SoundCloudProvider>,
    youtube: UseYouTube,
    youtube_playlist: Option<YouTubePlaylist>,
    local: UseLocalLibrary,
    playlists: UsePlaylists,
    downloads: UseDownloads,
    library_root: Option<PathBuf>,
    mut open: Signal<bool>,
    mut step: Signal<ImportStep>,
    mut busy: Signal<bool>,
    mut status: Signal<Option<String>>,
) {
    let Some(provider) = provider else {
        return;
    };
    let existing = current
        .iter()
        .filter(|choice| choice.already_imported)
        .count();
    busy.set(true);
    status.set(None);

        match provider {
            ImportProvider::Spotify => {
                let selected = current
                    .into_iter()
                    .filter(|choice| choice.selected && !choice.already_imported)
                    .map(|choice| SpotifyPlaylistSummary {
                        id: choice.source_id,
                        name: choice.name,
                        cover_url: choice.cover_url,
                        track_count: choice.track_count,
                    })
                    .collect();
                spawn(async move {
                    let outcome: Result<(usize, usize, usize, usize), String> = async {
                        let result = spotify
                            .playlists_for_import(selected)
                            .await
                            .map_err(|error| format!("Spotify: {error}"))?;
                        let skipped_playlists =
                            result.skipped_playlists + catalog_skipped;
                        let skipped_items = result.skipped_items;
                        let added = playlists.import_external(
                            "spotify",
                            result
                                .playlists
                                .into_iter()
                                .map(|playlist| PlaylistImport {
                                    source_id: playlist.id,
                                    name: playlist.name,
                                    tracks: playlist.tracks,
                                })
                                .collect(),
                        );
                        Ok((added, existing, skipped_playlists, skipped_items))
                    }
                    .await;
                    busy.set(false);
                    match outcome {
                        Ok((added, existing, skipped_playlists, skipped_items)) => {
                            status.set(Some(import_message(
                                added,
                                existing,
                                skipped_playlists,
                                skipped_items,
                            )));
                            step.set(ImportStep::Complete);
                        }
                        Err(error) => status.set(Some(error)),
                    }
                });
            }
            ImportProvider::SoundCloud => {
                let selected = current
                    .into_iter()
                    .filter(|choice| choice.selected && !choice.already_imported)
                    .map(|choice| SoundCloudPlaylistSummary {
                        id: choice
                            .source_id
                            .parse()
                            .expect("SoundCloud ids came from u64"),
                        title: choice.name,
                        cover_url: choice.cover_url,
                        track_count: choice.track_count,
                    })
                    .collect();
                spawn(async move {
                    let outcome: Result<(usize, usize, usize, usize), String> = async {
                        let result = soundcloud
                            .playlists_for_import(selected)
                            .await
                            .map_err(|error| format!("SoundCloud: {error}"))?;
                        let skipped_items = result.skipped_items;
                        let added = playlists.import_external(
                            "soundcloud",
                            result
                                .playlists
                                .into_iter()
                                .map(|playlist| PlaylistImport {
                                    source_id: playlist.id.to_string(),
                                    name: playlist.title,
                                    tracks: playlist.tracks,
                                })
                                .collect(),
                        );
                        Ok((added, existing, 0, skipped_items))
                    }
                    .await;
                    busy.set(false);
                    match outcome {
                        Ok((added, existing, skipped_playlists, skipped_items)) => {
                            status.set(Some(import_message(
                                added,
                                existing,
                                skipped_playlists,
                                skipped_items,
                            )));
                            step.set(ImportStep::Complete);
                        }
                        Err(error) => status.set(Some(error)),
                    }
                });
            }
            ImportProvider::YouTube => {
                let Some(playlist) = youtube_playlist else {
                    busy.set(false);
                    status.set(Some("YouTube: Load a playlist first.".into()));
                    return;
                };
                youtube.import_playlist(
                    playlist,
                    local,
                    playlists,
                    downloads,
                    library_root,
                );
                busy.set(false);
                open.set(false);
            }
        }
}
```

The root-owned YouTube hook and existing global download toast report
completion after the dialog closes. No dialog task polls a background process.

- [ ] **Step 7: Add dialog semantics and exact visible structure**

After the open/reset effect, derive render values and return this RSX. This is
the complete component tail; the event closures call the helpers from Steps 5
and 6 directly:

```rust
let is_open = *open.read();
let busy_value = *busy.read();
let step_value = *step.read();
let provider_value = *provider.read();
let status_value = status.read().clone();
let current_choices = choices.read().clone();
let selected_count = current_choices
    .iter()
    .filter(|choice| choice.selected && !choice.already_imported)
    .count();
let first_selectable = current_choices
    .iter()
    .find(|choice| !choice.already_imported)
    .map(|choice| choice.source_id.clone());
let spotify_connected = spotify.is_connected();
let youtube_busy = *youtube.busy.read();
let (source_heading, source_copy, source_placeholder, source_icon) =
    match provider_value {
        Some(ImportProvider::SoundCloud) => (
            "SoundCloud link",
            "Paste a public profile or playlist link. A profile shows all of its public playlists.",
            "https://soundcloud.com/artist-or-playlist",
            "fa-brands fa-soundcloud",
        ),
        _ => (
            "YouTube playlist link",
            "Paste one playlist link. Nira downloads its available entries as MP3 files.",
            "https://youtube.com/playlist?list=…",
            "fa-brands fa-youtube",
        ),
    };
let provider_icon = match provider_value {
    Some(ImportProvider::Spotify) => "fa-brands fa-spotify",
    Some(ImportProvider::SoundCloud) => "fa-brands fa-soundcloud",
    _ => "fa-brands fa-youtube",
};

rsx! {
    div {
        class: if is_open {
            "yt-downloader playlist-import open"
        } else {
            "yt-downloader playlist-import"
        },
        onkeydown: move |event: Event<KeyboardData>| {
            if event.key() == Key::Escape && !*busy.peek() {
                event.prevent_default();
                open.set(false);
            }
        },
        button {
            class: "yt-downloader-backdrop playlist-import-backdrop",
            r#type: "button",
            tabindex: "-1",
            "aria-hidden": "true",
            disabled: busy_value,
            onclick: move |_| open.set(false),
        }
        section {
            class: "yt-downloader-panel playlist-import-panel",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "playlist-import-title",
            header { class: "yt-downloader-head playlist-import-head",
                div {
                    h2 { id: "playlist-import-title", "Import playlists" }
                    p { "Choose a provider, then choose exactly what Nira should import." }
                }
                button {
                    class: "yt-downloader-close playlist-import-close",
                    r#type: "button",
                    title: "Close",
                    "aria-label": "Close playlist importer",
                    disabled: busy_value,
                    onclick: move |_| open.set(false),
                    i { class: "fa-solid fa-xmark" }
                }
            }

            if let Some(message) = status_value.as_ref() {
                p {
                    class: "playlist-import-status",
                    role: "status",
                    "aria-live": "polite",
                    "{message}"
                }
            }

            match step_value {
                ImportStep::Provider => rsx! {
                    div { class: "playlist-import-providers",
                        button {
                            class: "playlist-import-provider",
                            r#type: "button",
                            "data-import-provider": "true",
                            autofocus: spotify_connected && !busy_value,
                            disabled: !spotify_connected || busy_value,
                            onclick: {
                                let spotify = spotify.clone();
                                move |_| {
                                    provider.set(Some(ImportProvider::Spotify));
                                    load_spotify_catalog(
                                        spotify.clone(),
                                        playlists,
                                        choices,
                                        step,
                                        busy,
                                        status,
                                        catalog_skipped,
                                    );
                                }
                            },
                            i { class: "fa-brands fa-spotify" }
                            strong { "Spotify" }
                            span { "Your owned and collaborative playlists" }
                            if !spotify_connected {
                                small { "Connect Spotify in Settings first." }
                            }
                        }
                        button {
                            class: "playlist-import-provider",
                            r#type: "button",
                            "data-import-provider": "true",
                            autofocus: !spotify_connected && !busy_value,
                            disabled: busy_value,
                            onclick: {
                                let soundcloud = soundcloud.clone();
                                move |_| {
                                    provider.set(Some(ImportProvider::SoundCloud));
                                    youtube_playlist.set(None);
                                    status.set(None);
                                    if let Some(raw_url) =
                                        config.peek().soundcloud_profile_url.clone()
                                    {
                                        source_url.set(raw_url.clone());
                                        load_soundcloud_catalog(
                                            raw_url,
                                            soundcloud.clone(),
                                            playlists,
                                            choices,
                                            step,
                                            busy,
                                            status,
                                            catalog_skipped,
                                        );
                                    } else {
                                        source_url.set(String::new());
                                        step.set(ImportStep::Source);
                                    }
                                }
                            },
                            i { class: "fa-brands fa-soundcloud" }
                            strong { "SoundCloud" }
                            span { "Your public profile or any public playlist link" }
                        }
                        button {
                            class: "playlist-import-provider",
                            r#type: "button",
                            "data-import-provider": "true",
                            disabled: busy_value || youtube_busy,
                            onclick: move |_| {
                                provider.set(Some(ImportProvider::YouTube));
                                youtube_playlist.set(None);
                                source_url.set(String::new());
                                status.set(None);
                                step.set(ImportStep::Source);
                            },
                            i { class: "fa-brands fa-youtube" }
                            strong { "YouTube" }
                            span { "One playlist link, downloaded through yt-dlp" }
                            if youtube_busy {
                                small { "A YouTube import is already running." }
                            }
                        }
                    }
                },
                ImportStep::Source => rsx! {
                    div { class: "playlist-import-source",
                        div {
                            h3 { "{source_heading}" }
                            p { class: "hint", "{source_copy}" }
                        }
                        div { class: "searchbar-row",
                            SearchBar {
                                icon: Some(source_icon.to_string()),
                                value: source_url.read().clone(),
                                placeholder: source_placeholder.to_string(),
                                autofocus: true,
                                on_input: move |value: String| source_url.set(value),
                                on_submit: {
                                    let soundcloud = soundcloud.clone();
                                    move |_| load_source_catalog(
                                        *provider.peek(),
                                        source_url.peek().clone(),
                                        soundcloud.clone(),
                                        youtube,
                                        playlists,
                                        choices,
                                        youtube_playlist,
                                        step,
                                        busy,
                                        status,
                                        catalog_skipped,
                                    )
                                },
                            }
                            button {
                                class: "sq-btn sq-btn-primary sq-md",
                                r#type: "button",
                                disabled: busy_value
                                    || source_url.read().trim().is_empty(),
                                onclick: {
                                    let soundcloud = soundcloud.clone();
                                    move |_| load_source_catalog(
                                        *provider.peek(),
                                        source_url.peek().clone(),
                                        soundcloud.clone(),
                                        youtube,
                                        playlists,
                                        choices,
                                        youtube_playlist,
                                        step,
                                        busy,
                                        status,
                                        catalog_skipped,
                                    )
                                },
                                if busy_value {
                                    i { class: "fa-solid fa-circle-notch fa-spin" }
                                    " Loading"
                                } else {
                                    "Load"
                                }
                            }
                        }
                    }
                },
                ImportStep::Select => rsx! {
                    div { class: "playlist-import-toolbar",
                        h3 { "Choose playlists" }
                        div {
                            button {
                                class: "sq-btn sq-btn-ghost sq-sm",
                                r#type: "button",
                                autofocus: first_selectable.is_none(),
                                disabled: busy_value,
                                onclick: move |_| {
                                    let mut next = choices.peek().clone();
                                    select_all(&mut next, true);
                                    choices.set(next);
                                },
                                "Select all"
                            }
                            button {
                                class: "sq-btn sq-btn-ghost sq-sm",
                                r#type: "button",
                                disabled: busy_value,
                                onclick: move |_| {
                                    let mut next = choices.peek().clone();
                                    select_all(&mut next, false);
                                    choices.set(next);
                                },
                                "Deselect all"
                            }
                        }
                    }
                    if current_choices.is_empty() {
                        p { class: "hint", "No importable playlists found." }
                    } else {
                        div { class: "playlist-import-list",
                            for choice in current_choices.iter() {
                                {
                                    let source_id = choice.source_id.clone();
                                    let autofocus = first_selectable.as_deref()
                                        == Some(choice.source_id.as_str());
                                    rsx! {
                                        label {
                                            key: "{choice.source_id}",
                                            class: if choice.already_imported {
                                                "playlist-import-row disabled"
                                            } else if choice.selected {
                                                "playlist-import-row selected"
                                            } else {
                                                "playlist-import-row"
                                            },
                                            input {
                                                r#type: "checkbox",
                                                checked: choice.selected,
                                                disabled: choice.already_imported || busy_value,
                                                autofocus,
                                                onchange: move |event: FormEvent| {
                                                    let mut next = choices.peek().clone();
                                                    if let Some(choice) = next
                                                        .iter_mut()
                                                        .find(|choice| {
                                                            choice.source_id == source_id
                                                        })
                                                    {
                                                        choice.selected = event.checked()
                                                            && !choice.already_imported;
                                                    }
                                                    choices.set(next);
                                                },
                                            }
                                            div { class: "playlist-import-cover",
                                                if let Some(cover_url) =
                                                    choice.cover_url.as_ref()
                                                {
                                                    img {
                                                        src: "{cover_url}",
                                                        alt: "",
                                                        loading: "lazy",
                                                        decoding: "async",
                                                    }
                                                } else {
                                                    i { class: "{provider_icon}" }
                                                }
                                            }
                                            div { class: "playlist-import-meta",
                                                strong { "{choice.name}" }
                                                if choice.already_imported {
                                                    span { "Already imported" }
                                                } else {
                                                    span {
                                                        "{choice.track_count} tracks"
                                                    }
                                                }
                                            }
                                            span { class: "playlist-import-count",
                                                "{choice.track_count}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "playlist-import-actions",
                        if matches!(
                            provider_value,
                            Some(ImportProvider::SoundCloud | ImportProvider::YouTube)
                        ) {
                            button {
                                class: "sq-btn sq-btn-ghost sq-sm",
                                r#type: "button",
                                disabled: busy_value,
                                onclick: move |_| {
                                    choices.set(Vec::new());
                                    youtube_playlist.set(None);
                                    status.set(None);
                                    catalog_skipped.set(0);
                                    step.set(ImportStep::Source);
                                },
                                "Use another link"
                            }
                        } else {
                            span {}
                        }
                        button {
                            class: "sq-btn sq-btn-primary sq-md",
                            r#type: "button",
                            disabled: selected_count == 0 || busy_value,
                            onclick: {
                                let spotify = spotify.clone();
                                let soundcloud = soundcloud.clone();
                                move |_| import_selected(
                                    *provider.peek(),
                                    choices.peek().clone(),
                                    *catalog_skipped.peek(),
                                    spotify.clone(),
                                    soundcloud.clone(),
                                    youtube,
                                    youtube_playlist.peek().clone(),
                                    local,
                                    playlists,
                                    downloads,
                                    config.peek().library_root.clone(),
                                    open,
                                    step,
                                    busy,
                                    status,
                                )
                            },
                            if busy_value {
                                i { class: "fa-solid fa-circle-notch fa-spin" }
                                " Importing"
                            } else {
                                "Import {selected_count}"
                            }
                        }
                    }
                },
                ImportStep::Complete => rsx! {
                    div { class: "playlist-import-actions",
                        button {
                            class: "sq-btn sq-btn-ghost sq-sm",
                            r#type: "button",
                            disabled: busy_value,
                            onclick: move |_| {
                                provider.set(None);
                                choices.set(Vec::new());
                                source_url.set(String::new());
                                youtube_playlist.set(None);
                                status.set(None);
                                catalog_skipped.set(0);
                                step.set(ImportStep::Provider);
                            },
                            "Import more"
                        }
                        button {
                            class: "sq-btn sq-btn-primary sq-md",
                            r#type: "button",
                            autofocus: true,
                            onclick: move |_| open.set(false),
                            "Done"
                        }
                    }
                },
            }
        }
    }
}
```

- [ ] **Step 8: Add minimal token-based CSS**

Reuse the existing `.yt-downloader*` overlay classes by keeping both class
names on the importer shell, backdrop, panel, header, and close button as shown
in Step 7. Add only the importer-specific layout below:

```css
.playlist-import-toolbar,
.playlist-import-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.playlist-import-status {
  margin: 0 0 16px;
  color: var(--sub);
  font-size: 0.78rem;
  line-height: 1.45;
}

.playlist-import-providers {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px;
}

.playlist-import-provider,
.playlist-import-row {
  appearance: none;
  border: none;
  border-radius: var(--r);
  background: var(--raise2);
  color: var(--text);
}

.playlist-import-provider {
  min-height: 126px;
  padding: 16px;
  display: grid;
  align-content: start;
  gap: 6px;
  text-align: left;
  cursor: pointer;
}

.playlist-import-provider > i {
  margin-bottom: 8px;
  color: var(--sub);
  font-size: 1.25rem;
}

.playlist-import-provider > strong,
.playlist-import-provider > span,
.playlist-import-provider > small {
  display: block;
}

.playlist-import-provider > span,
.playlist-import-provider > small {
  color: var(--sub);
  font-size: 0.74rem;
  line-height: 1.4;
}

.playlist-import-provider:hover,
.playlist-import-row.selected {
  background: var(--raise3);
}

.playlist-import-provider:disabled,
.playlist-import-row.disabled {
  color: var(--faint);
  cursor: default;
}

.playlist-import-list {
  display: grid;
  gap: 6px;
  margin: 12px 0 16px;
}

.playlist-import-toolbar h3 {
  margin: 0;
}

.playlist-import-actions {
  margin-top: 16px;
}

.playlist-import-row {
  display: grid;
  grid-template-columns: auto 44px minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  padding: 8px;
}

.playlist-import-row input {
  width: 16px;
  height: 16px;
  accent-color: var(--text);
}

.playlist-import-row input:focus-visible {
  outline: 2px solid var(--text);
  outline-offset: 2px;
}

.playlist-import-cover {
  width: 44px;
  height: 44px;
  display: grid;
  place-items: center;
  overflow: hidden;
  border-radius: var(--rs);
  background: var(--raise1);
  color: var(--faint);
}

.playlist-import-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.playlist-import-meta {
  min-width: 0;
}

.playlist-import-meta strong,
.playlist-import-meta span {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.playlist-import-meta span,
.playlist-import-count {
  color: var(--sub);
  font-size: 0.74rem;
  font-variant-numeric: tabular-nums;
}

.playlist-import-source {
  display: grid;
  gap: 12px;
}

.playlist-import-source .searchbar-row {
  max-width: none;
}

.playlist-import-source .searchbar {
  max-width: none;
  border: none;
  background: var(--raise2);
}

@media (max-width: 620px) {
  .playlist-import-providers { grid-template-columns: 1fr; }
  .playlist-import-source .searchbar-row {
    flex-direction: column;
  }
  .playlist-import-row {
    grid-template-columns: auto 40px minmax(0, 1fr);
  }
  .playlist-import-count { grid-column: 3; }
  .playlist-import-toolbar,
  .playlist-import-actions {
    align-items: stretch;
    flex-direction: column;
  }
}
```

The shared overlay already provides narrow-width sizing and reduced-motion
handling. Do not duplicate it or add borders, shadows, gradients,
provider-colored cards, pill shapes, new fonts, or decorative animation.

- [ ] **Step 9: Run tests and compile check**

Run: `anvil tests`

Expected: PASS, including the selection helper test.

Run: `anvil check`

Expected: PASS; the old direct Spotify caller is gone and all provider DTOs resolve through `hooks`.

- [ ] **Step 10: Commit**

```bash
git add hooks/src/use_playlists.rs pages/src/library_import.rs pages/src/lib.rs pages/src/library.rs nira/assets/css/library.css
git commit -m "Add selective playlist import dialog"
```

### Task 7: Integration, failure-path, and visual verification

**Files:**
- Modify only files required by failures found in this task

**Interfaces:**
- Consumes: all prior tasks
- Produces: verified multi-provider import flow on `public`

- [ ] **Step 1: Confirm live-test prerequisites**

Build and synchronize the desktop bundle first:

```bash
anvil dev
```

Then confirm the standalone runtime prerequisites on the machine that will
launch the synchronized build:

```bash
command -v yt-dlp
command -v ffmpeg
test -x target/dx/nira/debug/linux/app/nira
test -f target/dx/nira/debug/linux/lib/libxdo.so.3
```

Launch the synchronized app from its asset directory, preserving the local
PATH that contains `yt-dlp` and `ffmpeg`:

```bash
(
  cd target/dx/nira/debug/linux/app
  LD_LIBRARY_PATH="../lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ./nira
)
```

Before claiming live provider verification, also confirm:

- Spotify has a configured client ID, a connected account, and the existing
  playlist-read scopes granted.
- At least one reachable public SoundCloud profile URL and one foreign public
  playlist URL are available.
- `config.library_root` points to a disposable test music folder.
- `yt-dlp` and `ffmpeg` resolve in the standalone app's runtime `PATH`; the
  `shell.nix` packages only guarantee availability during named Anvil tasks.

If a credential, public fixture, or runtime binary is unavailable, record that
manual case as blocked in the task handoff. Do not fake or silently omit it;
the automated gates below remain mandatory.

- [ ] **Step 2: Run the complete automated gate**

Run: `anvil tests`

Expected: all workspace tests pass.

Run: `anvil check`

Expected: workspace compile check passes.

Run: `git diff --check`

Expected: no whitespace errors.

- [ ] **Step 3: Verify Spotify against a connected account**

In the real app:

1. Open Library → Playlists → Import.
2. Confirm Spotify is disabled with a Settings explanation when disconnected.
3. Connect Spotify, reopen the importer, and choose Spotify.
4. Confirm playlist summaries appear before any import begins.
5. Confirm `Select all` and `Deselect all` affect only non-imported rows.
6. Select two playlists, import them, and verify only those two appear in Library → Playlists.
7. Rename one imported playlist locally, repeat discovery, and verify it is marked `Already imported` and the rename is preserved.
8. Right-click one imported track, add it to a separate Nira playlist, and
   verify the copied entry retains Spotify playback metadata.
9. Delete an imported playlist with the existing two-click delete action,
   reopen Import, and verify the same source playlist is selectable and can be
   imported again.
10. Confirm unavailable/non-track counts are reported without dropping successful playlists.

- [ ] **Step 4: Verify SoundCloud profile and foreign-link paths**

1. Save a public SoundCloud profile URL in Settings → Connections.
2. Choose SoundCloud in the importer and confirm its public playlist catalog loads without another prompt.
3. Use `Deselect all`, select one playlist, import it, and play at least one imported track.
4. Reopen, choose `Use another link`, paste another public profile URL, and verify multiple selectable rows.
5. Paste one public playlist permalink and verify exactly one selectable row.
6. Paste an HTTP URL, a lookalike host, a track URL, and a deleted/private playlist URL; verify each yields a readable error without closing or corrupting the dialog.

- [ ] **Step 5: Verify YouTube playlist download and recovery**

1. With no local music folder configured, submit a valid YouTube playlist and verify import fails with the existing Settings → Library instruction before starting a download.
2. Configure a music folder and import a short playlist containing at least one unavailable/private video.
3. Navigate away while it downloads; confirm the global download status continues.
4. Return to Library and verify one Nira playlist exists, playable local tracks are in source order, and the skipped count is reported.
5. Reopen the same link and verify the playlist is marked `Already imported`.
6. Paste a single-video link, HTTP link, and lookalike host; verify each is rejected before download.

- [ ] **Step 6: Inspect real UI in both themes and narrow width**

Capture and inspect the actual rendered provider chooser and selection list in dark and light themes at normal width and at approximately 560 px:

- Provider cards remain grayscale and readable.
- No borders, shadows, gradients, hue accents, pills, or clipped horizontal content appear.
- The panel uses `var(--r)` and controls use `var(--rs)`.
- Exactly one primary action is visible per step.
- Native checkbox focus/checked/disabled states are visible in both themes.
- Keyboard escape closes only while idle; initial focus lands on the first enabled provider.
- Status changes are announced through the live region.
- Reduced-motion mode removes panel/backdrop transitions without hiding state changes.

- [ ] **Step 7: Inspect the final diff and commit fixes**

Run:

```bash
git status --short
git diff --stat
git diff --check
```

Expected: only files named by this plan, plus narrowly justified fixes from verification, are changed.

If verification required changes:

Interactively stage only the verified fixes, inspect the staged diff, and commit:

```bash
git add -p
git diff --cached --check
git diff --cached --stat
git commit -m "Fix playlist import integration"
```

If no changes were required, do not create an empty commit.

## API References

- Spotify current-user playlists and 50-item pagination: <https://developer.spotify.com/documentation/web-api/reference/get-a-list-of-current-users-playlists>
- Spotify playlist items, current owner/collaborator restriction, and 50-item pagination: <https://developer.spotify.com/documentation/web-api/reference/get-playlists-items>
- SoundCloud URL resolution, playlist endpoints, and cursor pagination: <https://developers.soundcloud.com/docs/api/>
- SoundCloud playlist track access filtering: <https://developers.soundcloud.com/blog/high-tier-content-in-the-soundcloud-api/>
- `yt-dlp` playlist extraction and output templates: <https://github.com/yt-dlp/yt-dlp/blob/master/README.md>
