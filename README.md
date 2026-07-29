# nira

Nira is the desktop music player I wanted on Linux: SoundCloud, Spotify and a
local music folder in one library and one queue.

It is my daily driver, but it is still early software. There are no packaged
releases yet and some setup is required.

## What it does

- Searches and plays public SoundCloud tracks without an account.
- Connects to Spotify for search, saved music and playback.
- Scans a local music folder with MP3, FLAC, M4A, OGG and WAV files.
- Imports playlists from Spotify, SoundCloud and YouTube.
- Keeps likes, playlists, queue and playback position locally.
- Builds recommendations across providers instead of locking discovery to one
  service.
- Supports MPRIS media controls on Linux, keyboard shortcuts and a fullscreen
  visualizer.
- Scrobbles to ListenBrainz when an account is configured.

Spotify playback uses librespot and requires Premium. You also need your own
Spotify Developer client ID. SoundCloud works out of the box.

## Run it

The included Nix shell has the native Linux dependencies, Dioxus CLI, `yt-dlp`
and `ffmpeg`.

```sh
git clone https://github.com/drvcvt/nira.git
cd nira
nix-shell
dx serve --platform desktop
```

For a release build:

```sh
nix-shell --run 'scripts/build-desktop.sh release'
cd target/dx/nira/release/linux/app
./nira
```

Nira is currently developed and tested on Linux. Dioxus supports macOS and
Windows too, but those builds are not maintained here yet.

## First launch

SoundCloud search and playback need no setup. Everything else is configured
inside **Settings**:

- **Spotify:** create an app at
  [developer.spotify.com](https://developer.spotify.com), add
  `http://127.0.0.1:7777/callback` as its redirect URI, then paste the client
  ID into Nira and connect.
- **Local music:** choose a library folder and run a scan. Nira rescans on
  demand; it does not watch the folder for changes yet.
- **ListenBrainz:** add a username and token to enable scrobbling and listening
  history on Home.
- **Last.fm:** an optional API key adds another recommendation source.

YouTube imports are downloaded as tagged MP3 files into the selected local
library. They require `yt-dlp` and `ffmpeg`, both of which are already present
in the Nix shell.

## Data

Nira uses normal XDG directories:

- `~/.config/nira/` contains settings, OAuth tokens, likes and playlists.
- `~/.cache/nira/` contains covers, recommendations, history, queue state and
  logs.

The config directory is user data. Back it up before editing files by hand.
The cache directory can be deleted and rebuilt.

## Development

This is a Rust workspace with Dioxus for the desktop UI. The larger pieces are
kept separate so playback, providers and UI state do not depend on one giant
application object.

```text
nira/                 desktop shell and CSS
components/           shared UI components
pages/                home, search, library, detail and settings pages
hooks/                reactive state and application workflows
player/               rodio and librespot playback
provider-soundcloud/  SoundCloud API and streams
provider-spotify/     Spotify OAuth, API and playback wiring
provider-local/       local file scanner and tag reader
discovery/            cross-provider recommendations
config/               persisted settings and user data paths
```

Run the workspace checks inside the Nix shell:

```sh
nix-shell --run 'cargo test --workspace'
nix-shell --run 'cargo check --workspace'
```

The repository also contains `anvil.toml` with the equivalent `tests`, `check`,
`dev` and `release` tasks for contributors using Anvil.

## License

Apache-2.0. See [LICENSE](LICENSE).

Bundled fonts, icons and the visualizer engine keep their own licenses. See
[THIRD-PARTY.md](THIRD-PARTY.md).
