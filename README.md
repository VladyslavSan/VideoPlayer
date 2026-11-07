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

Click **Jellyfin** and sign in to your server. Servers on your network are found
automatically; otherwise type the address (`192.168.1.10:8096`, or a full `https://` URL).
Only the access token is stored, never your password.

- Movies and episodes appear in the same library as local folders, with the server's artwork.
- Files are direct-played (no transcoding), so HDR and hardware decoding work as they do for local files.
- Playback start, progress and stop are reported to the server, so resume positions and watched
  state stay in sync with other Jellyfin apps.

`cargo test -- --ignored` runs a round trip against Jellyfin's public demo server.

## Library

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
- `cargo run -- <file>`: play a file directly, without the library

## Licensing

The prebuilt libmpv in `scripts/fetch-libmpv.sh` is a GPL build.
