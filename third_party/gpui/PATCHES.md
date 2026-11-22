# GPUI 0.2.2, patched

A copy of [`gpui` 0.2.2 from crates.io](https://crates.io/crates/gpui/0.2.2), used through
`[patch.crates-io]` in the top-level `Cargo.toml`. The `examples` folder and its `[[example]]`
targets are left out. Changes:

- `src/platform/windows/direct_write.rs` (`create_glyph_run_analysis`): glyphs are rasterized
  with `NATURAL_SYMMETRIC` and grid fitting off, instead of the font's recommended mode. For
  Segoe UI at 12-15 px that was `NATURAL` with grid fitting, which antialiases only
  horizontally and looks jagged.
- `src/frame_capture.rs` (new) and `src/platform/windows/directx_renderer.rs` (`draw`,
  `save_frame`): `gpui::frame_capture::capture_next_frame(path)` saves the next frame drawn as a
  PNG, read back from the swap chain. Used by the app's test mode, whose window is never on
  screen, so the screen can't be captured.
- `src/taffy.rs`: `0.0_f32` / `1.0_f32` literals, so the build has no float-fallback warnings
  with newer Rust.
- `src/elements/uniform_list.rs`: items and decorations no longer add the list's top padding
  twice, and `ScrollStrategy::Top` scrolls an item to just below the padding (the app's grid
  scrolls under a see-through bar).
- Smooth mouse wheel scrolling: `src/elements/glide.rs` (new). A wheel step (`ScrollDelta::Lines`)
  in a scrolling element (`div.rs`, `paint_scroll_listener`) is no longer added to the offset at
  once; it's added to what the element still has to go, and each frame covers part of the rest,
  eased out over about 150 ms (`clamp_scroll_position` steps it before clamping and asks for
  another frame). It stops at an end, and when something else moves the offset (a touchpad, the
  app). Touchpads (`ScrollDelta::Pixels`) scroll as before.
  `gpui::set_smooth_wheel_scrolling(false)` turns it off (the app does, for reduced motion), and
  `gpui::set_wheel_glide_time` changes how long a step takes (an app setting).
- Deferred elements keep their parents' opacity: `src/window.rs` (`DeferredDraw.opacity`, set in
  `defer_draw`, applied in `prepaint_deferred_draws` and `paint_deferred_draws`) and
  `src/elements/div.rs` (`Interactivity::prepaint` applies the element's opacity while
  prepainting too, so `defer_draw` sees it). Upstream, a bar drawn with `deferred` inside a page
  fading in stayed fully opaque.
- Backdrop blur (frosted glass), Windows only: `gpui::backdrop_blur(radius)` is a background
  that blurs whatever was painted under the element.
  - `src/color.rs`: `BackgroundTag::BackdropBlur` (radius in `gradient_angle_or_pattern_height`,
    strength in `solid.a`) and `backdrop_blur()`.
  - `src/window.rs` (`paint_quad`): scales the radius to device pixels.
  - `src/scene.rs` (`insert_primitive`): a blur's draw order covers everything within its reach
    (3 × radius), not only under it.
  - `src/style.rs`: the new tag in the border-colour match.
  - `src/platform/windows/directx_renderer.rs`: quad batches are drawn in runs between blurs.
    A run of blurs copies the frame once, then each draws a horizontal pass into a scratch
    texture and a vertical pass back into the frame (clipped to its rounded bounds). The
    textures are made on first use and dropped on resize.
  - `src/platform/windows/shaders.hlsl` (`backdrop_blur_*`) and `build.rs` (module list).
  - The other renderers (Metal, Linux) don't know the tag, so the app only uses it on
    Windows.

To update GPUI, copy the new release over this folder and apply the same changes again.
