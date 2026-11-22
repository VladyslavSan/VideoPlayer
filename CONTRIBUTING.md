# Conventions

## Interface principles

The app should feel calm and continuous, like a native media app: nothing on screen jumps.

- **Smooth scrolling.** Every scrolling view moves smoothly. A mouse wheel step glides (the
  patched GPUI does this for any `overflow_scroll` element and `uniform_list`); scrolling the
  app does itself (to a row, to the top, keyboard moves through a grid) eases there too.
- **Smooth transitions.** Things appear, disappear, open, close and change size with an
  animation, not a cut: pages rise and fade in, sections fold, overlays and bars fade. State
  that swaps in place (a poster loading, a badge changing) fades rather than pops.
- **Standard timings.** Use the constants in `src/motion.rs` (`PAGE`, `FOLD`, …) and its
  `ease_out`, and add a named constant there for a new kind of motion instead of a literal in
  place. Roughly 150–250 ms, easing out; long enough to follow, short enough never to wait on.
- **Reduced motion.** Animations run only when `motion::enabled()`: the system's "Animation
  effects" (Windows) / "Reduce motion" (macOS) turns them off, and so does
  `VIDEO_PLAYER_NO_MOTION`. Without motion, things simply appear in their final state.
- **No layout jumps.** Space for something that's still loading is reserved (placeholders the
  size of the real thing), so the page doesn't shift when it arrives.
- **Smooth frames.** Rendering a frame must stay cheap: no per-frame work that grows with the
  library (index or cache it), virtualize long lists (`uniform_list`), and keep network and disk
  off the UI thread.

## Commits

Commit messages follow [Conventional Commits 1.0](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>(<scope>): <summary>

<body>

<footers>
```

- **Summary**: imperative, lower case (names like Sonarr keep their capitals), no full stop, at
  most 72 characters in all (`fix(arr): decode split-scan JPEG posters`, not
  `Fixed the posters.`).
- **Body** (optional, after a blank line): what was wrong or missing and why the change fixes
  it, in plain words, wrapped at 72 columns.
- **Footers**: `BREAKING CHANGE: ...`, `Co-Authored-By: ...` and the like, after a blank line.
- One logical change per commit; if the summary needs "and", it's probably two commits.

### Types

| Type       | For                                                                        |
| ---------- | -------------------------------------------------------------------------- |
| `feat`     | Something new the user can see or do                                       |
| `fix`      | A bug fix                                                                  |
| `perf`     | Faster or lighter, same behaviour                                          |
| `refactor` | Code changes that neither fix a bug nor add a feature                      |
| `test`     | Tests only                                                                 |
| `docs`     | README, CONTRIBUTING, comments-only changes                                |
| `build`    | `build.rs`, `build/`, Cargo manifests and dependencies, libmpv fetching    |
| `ci`       | `.github/workflows`, `scripts/ci-*`                                        |
| `chore`    | Anything else that doesn't touch the app (tooling, formatting, cleanup)    |
| `revert`   | Reverting an earlier commit (`revert: <its summary>`, body names the hash) |

A change that breaks compatibility (settings or library database that older builds can't read,
removed keyboard shortcuts, …) adds `!` after the type or scope (`feat(library)!: …`) and a
`BREAKING CHANGE:` footer saying what users have to do.

### Scopes

Optional, but use one when the change sits in one area:

| Scope      | Covers                                                    |
| ---------- | --------------------------------------------------------- |
| `player`   | Playback, the player screen and its controls (`src/player`, `src/video`) |
| `library`  | Local folders, scanning, the library database (`src/library`) |
| `jellyfin` | Jellyfin sign-in, browsing, sync (`src/jellyfin`)         |
| `arr`      | Sonarr and Radarr (`src/arr`, their pages)                |
| `ui`       | Shell, navigation, settings, shared widgets (`src/shell`, `src/ui`) |
| `media`    | System media controls (`src/media_session`)               |
| `gpui`     | The patched GPUI in `third_party/gpui`                    |
| `mac`      | macOS-only changes (bundling, platform code)              |
| `windows`  | Windows-only changes (installer script, platform code)    |

New scopes are fine when none of these fit; keep them short and lower case.

### Examples

```
feat(arr): list everything Sonarr and Radarr track
fix(arr): decode split-scan JPEG posters
feat(gpui): glide mouse wheel scrolling
perf(ui): match download titles through an index
build: fetch the official LGPL libmpv on Windows
ci: play the test clips instead of skipping them
```

### Checking messages

`scripts/check-commits.sh` checks messages against these rules: CI runs it on every push and
pull request (`.github/workflows/checks.yml`), and a local hook runs it as you commit. Enable
the hook once per clone:

```
git config core.hooksPath scripts/hooks
```

## Code style

- `cargo fmt` formatting, default settings.
- No clippy warnings: `cargo clippy --all-targets -- -D warnings`.
- CI checks both on every push and pull request (`.github/workflows/checks.yml`); run them
  before committing.
- `third_party/gpui` is a patched copy of upstream code and keeps upstream's style; list every
  change to it in `third_party/gpui/PATCHES.md`.
