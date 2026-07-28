use std::{path::PathBuf, sync::Arc};

use components::SearchBar;
use dioxus::prelude::*;
use hooks::{
    PlaylistImport, SoundCloudPlaylistSummary, SoundCloudProvider, SpotifyPlaylistSummary,
    SpotifyProvider, UseDownloads, UseLocalLibrary, UsePlaylists, UseYouTube, YouTubePlaylist,
    use_config, use_downloads, use_local_library, use_playlists, use_soundcloud, use_spotify,
    use_youtube,
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
            if skipped_items == 1 {
                "item was"
            } else {
                "items were"
            }
        ));
    }
    parts.join(" ")
}

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

fn youtube_choices(playlists: UsePlaylists, playlist: &YouTubePlaylist) -> Vec<ImportChoice> {
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
            let selected: Vec<SpotifyPlaylistSummary> = current
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
                    let skipped_playlists = result.skipped_playlists + catalog_skipped;
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
            let selected: Vec<SoundCloudPlaylistSummary> = current
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
                        .playlists_for_import(&selected)
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
            youtube.import_playlist(playlist, local, playlists, downloads, library_root);
            busy.set(false);
            open.set(false);
        }
    }
}

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
    let busy = use_signal(|| false);
    let mut status = use_signal(|| None::<String>);
    let mut catalog_skipped = use_signal(|| 0usize);
    let mut was_open = use_signal(|| false);

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
    let (source_heading, source_copy, source_placeholder, source_icon) = match provider_value {
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
                                    disabled: busy_value || source_url.read().trim().is_empty(),
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
                                                        span { "{choice.track_count} tracks" }
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
