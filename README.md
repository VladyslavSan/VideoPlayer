# Video Player

An Infuse-style video player for Windows and macOS, built with Rust, [GPUI](https://gpui.rs) and libmpv.

## Download

[Releases](../../releases) has a zip for each platform:

- **Windows** (x64): unzip anywhere and run `video-player.exe` (keep `libmpv-2.dll` next to it).
- **macOS** (Apple Silicon and Intel, macOS 11+): unzip and move **Video Player** to
  Applications. The app isn't notarized by Apple, so the first time, right-click it and choose
  **Open** (or run `xattr -dr com.apple.quarantine "/Applications/Video Player.app"`).

## Setup

Windows (Git Bash, with Visual Studio's C++ tools):

```sh
bash scripts/fetch-libmpv.sh   # downloads libmpv into third_party/mpv
cargo run -- "D:\path\to\movie.mkv"
```

macOS (Xcode):

```sh
bash scripts/fetch-libmpv-macos.sh   # downloads libmpv into third_party/mpv
cargo run -- ~/Movies/movie.mkv
```

## Architecture

The app uses a single window, with the video below GPUI's UI.

- **Windows:** mpv renders into a disabled child window, and GPUI's DirectComposition layer,
  which Windows composites above child windows, draws the UI on top. mpv keeps its own swapchain,
  so hardware decoding and HDR output are preserved.
- **macOS:** mpv's `--wid` embedding isn't available there, so mpv draws through its render API
  into an OpenGL layer in a view below GPUI's Metal view, which is transparent wherever the UI
  doesn't draw. Decoding still uses VideoToolbox. libmpv comes from
  [media-kit's builds](https://github.com/media-kit/libmpv-darwin-build) (universal, with all its
  libraries in the app).

## Jellyfin

Click **Jellyfin** and sign in to your server. Servers that answer Jellyfin's network
discovery are listed automatically; otherwise type the address (`192.168.1.10:8096`, a full
`https://` URL, or a link copied from Jellyfin's web app).

Your password is never stored. The access token Jellyfin issues goes into the system
credential store (Windows Credential Manager or the macOS Keychain), which
encrypts it for your user account; `library.db` keeps only the server address and user name.
**Sign Out** revokes the token on the server and deletes it locally.

- Movies and episodes appear in the same library as local folders, with the server's artwork.
- Files are direct-played by default, so HDR and hardware decoding work as they do for local files.
  For slow or remote connections, pick a **streaming quality** (40 down to 1.5 Mbps) from a menu:
  per server on the Jellyfin page ("Streaming: …"), or while watching under Options in the
  audio/subtitles panel, which switches at once and keeps your place. The server then transcodes to HLS; text
  subtitles are fetched as separate files (picture-based ones like PGS aren't available then).
- Playback start, progress and stop are reported to the server, so resume positions and watched
  state stay in sync with other Jellyfin apps.

`cargo test -- --ignored` runs a round trip against Jellyfin's public demo server.

## Player

A floating control bar appears when you move the mouse and hides after a few seconds of
playback (the pointer hides with it). Once hidden, it only comes back for a deliberate move
(about 40 px within half a second), not a nudge. While it's showing, subtitles move up so the bar never
covers them. The subtitles button opens a panel with audio tracks, subtitle tracks (including
external subtitle files on a Jellyfin server), playback speed, subtitle size and subtitle delay.
The volume is remembered between videos. The timeline is split at chapter starts, with intros, recaps and credits highlighted;
hovering it shows the time and the chapter's name. It also shows a picture of that moment: a second,
hidden player opens the same video on the first hover and snapshots the spot (one picture per 10
seconds, kept until the video closes). Not for transcoded Jellyfin streams, which are meant for
slow connections.

For TV shows, the bar adds **Next episode** and **Episodes** (a season-by-season list). When the
credits start (or the episode ends), an "Up next" card counts down and plays the next one,
staying in fullscreen; **Watch credits** dismisses it. During an intro or recap a **Skip Intro**
button appears. Intros, recaps and credits come from Jellyfin's media segments (Jellyfin 10.10+
with a detection plugin such as Intro Skipper); for anything the server has no markers for, from the
community databases [TheIntroDB](https://theintrodb.org) and [IntroDB](https://introdb.app),
looked up by the show's TVDB/TMDB/IMDb ids (only those ids, the season and episode number and the
runtime are sent); or else from chapter names like "Opening" or "Credits".

Audio and subtitle choices are remembered per show and as a general preference (including
"subtitles off"), and applied when the next video starts.

The player shows up in the system's media controls (the Windows media flyout, the macOS Now
Playing menu) with the title and artwork. Headphone buttons and keyboard media keys play and
pause, even while another window is in front; next and previous switch episodes, and the
flyout's timeline seeks. To keep headphone skip buttons from changing episodes, turn off
**Settings → Next and previous switch episodes**; play and pause keep working.

The Jellyfin library is cached on disk, so it shows instantly at start and stays browsable when
the server is offline.
Press **F1** (or `?`) in the player to see all shortcuts.

| Key | Action |
| --- | --- |
| Space / K | Play or pause |
| ← → / J L | Back or forward 10 seconds |
| ↑ ↓ | Volume |
| M | Mute |
| S / A | Next subtitle track (including off) / next audio track |
| C | Audio and subtitles panel |
| [ ] | Slower / faster |
| Z / X | Subtitles earlier / later by 0.1 s |
| F, double-click | Fullscreen |
| Click | Show or hide the controls (clicking never pauses) |
| N / E | Next episode / episode list |
| Enter | Skip intro, or play the next episode now |
| → / L during "Up next" | Play the next episode now |
| F1, ? | Keyboard shortcuts |
| Esc | Close the panel, leave fullscreen, or go back to the library |
| Backspace | Back to the library |

## Library

The arrow keys move a ring between cards and Enter opens the one it's on (on a movie's page,
Enter plays it); Esc goes back, with the ring where you left it. From the search field, Down
moves into the results.

**Home** shows Continue Watching (including the next episode of shows you're watching) and the
latest additions to each library. Clicking an episode's picture plays it; clicking the text under
it opens its show at that season, with the episode highlighted. Each Jellyfin library (Movies,
Shows, Anime Series, ...) gets its own tab, in the server's order; local folders get **Movies** and
**TV Shows** tabs. The tabs are full grids with sorting (title, recently added, recently watched,
year); they only draw the rows on screen, so large libraries stay fast. **Search** (Ctrl+F, ⌘F on a Mac) matches
movies, shows and episode titles; Esc clears it.

Selecting a movie opens its details: backdrop, year, runtime, rating, genres, overview, the
video/audio/subtitle tracks (from Jellyfin), **Play** or **Resume**, **Play from beginning** and
**Mark as watched/unwatched** (synced to the server). A show's page starts with its backdrop,
overview and a button for the episode to watch next.

Add folders with **Add Folder**. They are scanned in the background, and movie and
episode names are parsed from file and folder names (`Title (Year)`, `Show.S01E02`,
`Show - 1x02`, `Show/Season 1/...`). Thumbnails are taken from a frame of each video.
Watch progress is saved every 5 seconds, so playback resumes where you left off.

Library data lives in `%LOCALAPPDATA%\VideoPlayer` on Windows and
`~/Library/Application Support/VideoPlayer` on macOS (the `library.db` file and the `thumbs`
folder).

## Development

- `cargo test`: parser, database and scanner tests
- `VIDEO_PLAYER_DATA_DIR=<dir>`: use a separate library, for example while testing
- `VIDEO_PLAYER_MPV_LOG=<path>`: write mpv's verbose log
- `VIDEO_PLAYER_MPV_OPTIONS="ao=null ..."`: extra mpv options (space-separated `key=value`)
- `cargo run -- <file>`: play a file directly, without the library
- `VIDEO_PLAYER_TEST_WINDOW=1`: for automated tests on a computer someone is using. The window
  is cloaked (drawn but never shown), never takes focus and stays out of the taskbar, and the
  app stays out of the system media controls (unless `VIDEO_PLAYER_TEST_MEDIA=1`). With
  `VIDEO_PLAYER_TEST_CAPTURE=<dir>`, writing a name into `<dir>/request` makes the app save
  `<name>.ui.png` (its own frame, transparent over the video) and `<name>.video.png` (mpv's);
  layering the first over the second gives what the screen would show.
- `python scripts/make_icon.py`: redraw `assets/app.ico` (embedded in the exe via `assets/app.rc`)

Release builds have no console window; their diagnostics go to `app.log` in the data folder
(the previous run's log is kept as `app.previous.log`).

## Releases

Pushing a version tag builds both apps on GitHub Actions, runs the tests on each, and publishes
a release with the two zips (`.github/workflows/release.yml`):

```sh
git tag v0.2.0 && git push origin v0.2.0
```

The tag sets the version; Cargo.toml's is replaced during the build. Running the workflow by
hand (Actions > Release > Run workflow) builds and tests without publishing.
`scripts/bundle-macos.sh` builds the macOS app locally.

## Install from source (Windows)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install.ps1
```

Builds a release, installs it to `%LOCALAPPDATA%\Programs\VideoPlayer` and adds a **Video Player**
Start Menu shortcut. Run it again to update; this works while the app is open, and the new
version starts next time. `scripts\install.ps1 -Uninstall` removes the app (your library and
settings stay).

## Licensing

The prebuilt libmpv in `scripts/fetch-libmpv.sh` (Windows) is a GPL build; the macOS one from
media-kit is LGPL.
