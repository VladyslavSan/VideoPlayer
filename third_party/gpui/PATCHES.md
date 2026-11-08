# GPUI 0.2.2, patched

A copy of [`gpui` 0.2.2 from crates.io](https://crates.io/crates/gpui/0.2.2), used through
`[patch.crates-io]` in the top-level `Cargo.toml`. The `examples` folder and its `[[example]]`
targets are left out. Changes:

- `src/platform/windows/direct_write.rs` (`create_glyph_run_analysis`): glyphs are rasterized
  with `NATURAL_SYMMETRIC` and grid fitting off, instead of the font's recommended mode. For
  Segoe UI at 12-15 px that was `NATURAL` with grid fitting, which antialiases only
  horizontally and looks jagged.
- `src/taffy.rs`: `0.0_f32` / `1.0_f32` literals, so the build has no float-fallback warnings
  with newer Rust.

To update GPUI, copy the new release over this folder and apply the same changes again.
