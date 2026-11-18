# Nocturne patch

This is `eframe` 0.37.0 from crates.io, by Emil Ernerfeldt and the egui contributors,
distributed under the MIT License or the Apache License 2.0 (see `Cargo.toml`). It is kept
here until an egui release sizes the iOS drawing surface from the full window (`outer_size`).

The change: on iOS the drawing surface is sized from the window's outer size, the same size
`egui-winit` lays out over (`egui_winit::screen_size_in_pixels`). Release 0.37.0 used the inner
size, which on iOS is only the safe area, so the picture was drawn too short and stretched
to the full screen. Pencil ink and taps then landed away from what was drawn.

`src/native/wgpu_integration.rs` uses `egui_wgpu::winit::surface_size` in the `Resized`
handler. The helper itself is in `vendor/egui-wgpu`.

Every other platform is unchanged: `surface_size` returns the inner size there.

When an egui release sizes iOS surfaces from the full window, remove this folder and its
`[patch.crates-io]` line in the workspace `Cargo.toml`.
