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

To update GPUI, copy the new release over this folder and apply the same changes again.
