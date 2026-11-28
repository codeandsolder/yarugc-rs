# Masonry Imaging

Masonry-owned imaging helpers and backend adapters.

This crate currently provides:

- shared helpers for preparing retained scenes from Masonry-style base content plus overlays
- a `vello` module backed by `imaging_vello`
- a `vello_hybrid` module backed by `imaging_vello_hybrid`
- a `vello_cpu` module backed by `imaging_vello_cpu`
- a `skia` module backed by `imaging_skia`

No concrete renderer is enabled by default. The native host selects a backend using `texture_render::Renderer::new(Backend)` and enables the matching Cargo feature.
