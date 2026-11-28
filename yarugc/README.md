# YARUGC — Yet Another Rust GUI Crate

Engine-owned retained UI. Masonry handles widget state, layout and events. A cached
window renders to a persistent transparent texture only when its root requests a
redraw. Native GPU producers can update other surfaces independently. A prepared
composition returned by the fallible `Compositor::prepare` reuses GPU placement buffers and texture bindings until geometry or
ordering changes. Preparation rejects mismatched devices and unsupported buffer sizes. The host draws it over its own scene and presents. The compositor does not select
a raster backend. Enable the optional `readouts` feature when the application
uses fixed-cell text; this activates the shared Vello Hybrid atlas.

## Shared text rendering

Ordinary text is shaped and positioned by Parley/HarfRust. When the host selects
Vello Hybrid, that renderer lazily rasterizes missing glyphs into bounded GPU atlas
pages. The same renderer supplies pinned glyph subsets to fixed-cell layers.
Use the same FontData identity, physical size and hinting policy to reuse entries.
Large or transformed outlines that cannot use the atlas remain GPU vector draws.

## Fixed-layout telemetry

- Resolve widget geometry with `CachedWindow::slot(widget_id)` after layout.
  `SlotGeometry` retains the content box, physical transform and ancestor clips.
  `Placement` supports arbitrary affine transforms and convex clipping.
- Create a `ReadoutRenderer` from the existing window renderer, shared
  `FontData`, `ReadoutStyle` and the desired alphabet. `ReadoutStyle` allows an
  explicit physical cell advance and variation coordinates, independent of the font’s natural spacing. `prepare` creates a retained `ReadoutBatch` for a window's slots.
- Use `ReadoutBatch::set_text` to update fixed-width, right-aligned glyph cells.
  It validates characters and capacity before mutation. No layout, shaping,
  allocation or geometry rebuilding is needed for a value update.
- `ReadoutBatch::render` clears and reconstructs damaged tiles in painter order
  into its persistent premultiplied surface; dense changes use a single full draw. Submit the encoder before changing values
  again. Ordinary complex/interactive text still uses Masonry/Parley.
- Compose the cached window surface, dynamic readout surface and any native GPU
  instrument surfaces in painter order. Reuse the prepared composition until
  placement/order/texture allocation changes. Reprepare slots after layout
  changes; prepare a new glyph subset when the desired physical font size changes.

Damage tracking is confined to the fixed-cell layer; ordinary window paint
is retained independently.
Window-local input routing, docking, and extraction of foreground/popup layers
above dynamic content are subsequent work. The compositor accepts straight or
premultiplied RGBA8 layers in painter order. Vello writes straight alpha; Hybrid writes premultiplied alpha. The
compositor premultiplies straight-alpha input before interpolation.

## Validation

On a machine with a working wgpu adapter:

```sh
cargo test -p yarugc --locked --features readouts -- --include-ignored --nocapture
```

The GPU regression fixtures verify composition over a host scene, alpha,
moving/clipping, resizing, and 256 visible Masonry grid readouts changing
magnitude/units without modifying layout or repainting the static surface.
The CPU timing printed by the readout test measures only value updates, not
formatting or GPU frame time.

### Optimized telemetry benchmark

```sh
cargo test -p yarugc --locked --features readouts --release --test host_gpu -- --ignored --nocapture
```

The 256-visible-readout fixture measures preformatted string updates for 1, 8,
64 and 256 changing slots. On devices supporting encoder timestamp queries it
also measures dynamic-layer rasterization and two-layer host composition over
64 submissions, reporting median and p95 GPU intervals separately. It excludes
3D work, presentation, text formatting and CPU-to-GPU staging upload time.

### Experimental renderer comparisons

The alternate renderer and glyph-cell experiments live in [renderer-comparisons](../benchmarks/renderer-comparisons/README.md), independent of the compositor's normal dependency and test graph.
