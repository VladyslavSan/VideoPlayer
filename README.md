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

Set `VIDEO_PLAYER_MPV_LOG=<path>` to write mpv's verbose log.

## Licensing

The prebuilt libmpv in `scripts/fetch-libmpv.sh` is a GPL build.
