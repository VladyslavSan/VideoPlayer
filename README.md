# Video Player

An Infuse-style video player for Windows, built with Rust, [GPUI](https://gpui.rs) and libmpv.

## Setup

```sh
bash scripts/fetch-libmpv.sh   # downloads libmpv into third_party/mpv (Git Bash)
cargo run -- "D:\path\to\movie.mkv"
```

## Architecture

The app uses a single window. mpv renders into a disabled child window, and GPUI's
DirectComposition layer, which Windows composites above child windows, draws the UI on top.
mpv keeps its own swapchain, so hardware decoding and HDR output are preserved.

## Jellyfin

Click **Jellyfin** and sign in to your server. Servers that answer Jellyfin's network
discovery are listed automatically; otherwise type the address (`192.168.1.10:8096`, a full
`https://` URL, or a link copied from Jellyfin's web app).

Your password is never stored. The access token Jellyfin issues goes into the system
credential store (Windows Credential Manager, macOS Keychain, Secret Service on Linux), which
encrypts it for your user account; `library.db` keeps only the server address and user name.
**Sign Out** revokes the token on the server and deletes it locally.

- Movies and episodes appear in the same library as local folders, with the server's artwork.
- Files are direct-played (no transcoding), so HDR and hardware decoding work as they do for local files.
- Playback start, progress and stop are reported to the server, so resume positions and watched
  state stay in sync with other Jellyfin apps.

`cargo test -- --ignored` runs a round trip against Jellyfin's public demo server.

## Player

A floating control bar appears when you move the mouse and hides after a few seconds of
playback (the pointer hides with it). While it's showing, subtitles move up so the bar never
covers them. The subtitles button opens a panel with audio tracks, subtitle tracks (including
external subtitle files on a Jellyfin server), playback speed, subtitle size and subtitle delay.
The volume is remembered between videos.

For TV shows, the bar adds **Next episode** and **Episodes** (a season-by-season list). When the
credits start (or the episode ends), an "Up next" card counts down and plays the next one,
staying in fullscreen; **Watch credits** dismisses it. During an intro or recap a **Skip Intro**
button appears. Intros and credits come from Jellyfin's media segments (Jellyfin 10.10+ with a
detection plugin such as Intro Skipper), or else from chapter names like "Opening" or "Credits".

Audio and subtitle choices are remembered per show and as a general preference (including
"subtitles off"), and applied when the next video starts.

The Jellyfin library is cached on disk, so it shows instantly at start and stays browsable when
the server is offline.
Press **F1** (or `?`) in the player to see all shortcuts.

| Key | Action |
| --- | --- |
| Space / K | Play or pause (click the video, too) |
| ← → / J L | Back or forward 10 seconds |
| ↑ ↓ | Volume |
| M | Mute |
| S / A | Next subtitle track (including off) / next audio track |
| C | Audio and subtitles panel |
| [ ] | Slower / faster |
| Z / X | Subtitles earlier / later by 0.1 s |
| F, double-click | Fullscreen |
| N / E | Next episode / episode list |
| Enter | Skip intro, or play the next episode now |
| F1, ? | Keyboard shortcuts |
| Esc | Close the panel, leave fullscreen, or go back to the library |
| Backspace | Back to the library |

## Library

The library has three tabs. **Home** shows Continue Watching (including the next episode of
shows you're watching), recently added movies and recently updated shows. **Movies** and
**TV Shows** are full grids with sorting (title, recently added, recently watched, year); they
only draw the rows on screen, so large libraries stay fast. **Search** (Ctrl+F) matches
movies, shows and episode titles; Esc clears it.

Selecting a movie opens its details: backdrop, year, runtime, rating, genres, overview, the
video/audio/subtitle tracks (from Jellyfin), **Play** or **Resume**, **Play from beginning** and
**Mark as watched/unwatched** (synced to the server). A show's page starts with its backdrop,
overview and a button for the episode to watch next.

Add folders with **Add Folder**. They are scanned in the background, and movie and
episode names are parsed from file and folder names (`Title (Year)`, `Show.S01E02`,
`Show - 1x02`, `Show/Season 1/...`). Thumbnails are taken from a frame of each video.
Watch progress is saved every 5 seconds, so playback resumes where you left off.

Library data lives in `%LOCALAPPDATA%\VideoPlayer` (the `library.db` file and the `thumbs`
folder).

## Development

- `cargo test`: parser, database and scanner tests
- `VIDEO_PLAYER_DATA_DIR=<dir>`: use a separate library, for example while testing
- `VIDEO_PLAYER_MPV_LOG=<path>`: write mpv's verbose log
- `VIDEO_PLAYER_MPV_OPTIONS="ao=null ..."`: extra mpv options (space-separated `key=value`)
- `cargo run -- <file>`: play a file directly, without the library
- `python scripts/make_icon.py`: redraw `assets/app.ico` (embedded in the exe via `assets/app.rc`)

Release builds have no console window; their diagnostics go to
`%LOCALAPPDATA%\VideoPlayer\app.log` (the previous run's log is kept as `app.previous.log`).

## Install

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install.ps1
```

Builds a release, installs it to `%LOCALAPPDATA%\Programs\VideoPlayer` and adds a **Video Player**
Start Menu shortcut. Run it again to update; this works while the app is open, and the new
version starts next time. `scripts\install.ps1 -Uninstall` removes the app (your library and
settings stay).

## Licensing

The prebuilt libmpv in `scripts/fetch-libmpv.sh` is a GPL build.
