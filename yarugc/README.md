# YARUGC — Yet Another Rust GUI Crate

Engine-owned retained UI. Masonry handles widget state, layout and events. A cached
window renders to a persistent transparent texture only when its root requests a
redraw. Native GPU producers can update other surfaces independently. A prepared
composition returned by the fallible `Compositor::prepare` reuses GPU placement buffers and texture bindings until geometry or
ordering changes. Preparation rejects mismatched devices and unsupported buffer sizes. The host draws it over its own scene and presents.

## Fixed-layout telemetry

- Resolve widget geometry with `CachedWindow::slot(widget_id)` after layout.
  `SlotGeometry` retains the content box, physical transform and ancestor clips.
  `Placement` supports arbitrary affine transforms and convex clipping.
- Create a shared `ReadoutRenderer` with font bytes, a pixel size and the desired
  alphabet. `prepare` creates a retained `ReadoutBatch` for a window's slots.
- Use `ReadoutBatch::set_text` to update fixed-width, right-aligned glyph cells.
  It validates characters and capacity before mutation. No layout, shaping,
  allocation or geometry rebuilding is needed for a value update.
- `ReadoutBatch::render` encodes a single draw into its persistent premultiplied
  surface when any glyph IDs change. Submit the encoder before changing values
  again. Ordinary complex/interactive text still uses Masonry/Parley.
- Compose the cached window surface, dynamic readout surface and any native GPU
  instrument surfaces in painter order. Reuse the prepared composition until
  placement/order/texture allocation changes. Reprepare slots after layout
  changes; regenerate the glyph atlas when the desired physical font size changes.

This architecture deliberately does not perform general-purpose damage tracking.
Window-local input routing, docking, and extraction of foreground/popup layers
above dynamic content are subsequent work. The compositor accepts straight or
premultiplied RGBA8 layers in painter order. Vello writes straight alpha; the
compositor premultiplies before interpolation.

## Validation

On a machine with a working wgpu adapter:

```sh
cargo test -p yarugc --locked -- --include-ignored --nocapture
```

The GPU regression fixtures verify composition over a host scene, alpha,
moving/clipping, resizing, and 256 visible Masonry grid readouts changing
magnitude/units without modifying layout or repainting the static surface.
The CPU timing printed by the readout test measures only value updates, not
formatting or GPU frame time.

### Optimized telemetry benchmark

```sh
cargo test -p yarugc --locked --release --test host_gpu -- --ignored --nocapture
```

The 256-visible-readout fixture measures preformatted string updates for 1, 8,
64 and 256 changing slots. On devices supporting encoder timestamp queries it
also measures dynamic-layer rasterization and two-layer host composition over
64 submissions, reporting median and p95 GPU intervals separately. It excludes
3D work, presentation, text formatting and CPU-to-GPU staging upload time.

### Text backend comparison and opaque cells

```sh
cargo test -p yarugc --locked --release --test text_backends -- --ignored --nocapture --test-threads=1
```

256 visible slots, 2048×512, Roboto Regular at 18 physical pixels/em, 48 samples,
pre-shaped text alternating between 5 and 7 characters per slot. Glyphon and
cached wgpu-font-renderer rasterize coverage in advance; the analytic modes
render glyph outlines each frame. CPU preparation excludes text shaping and
formatting. GPU timestamps exclude staging uploads and composition; Vello is
bracketed across submissions and can include scheduling gaps. Exact v1.1.0
Git revision avoids CRLF shader packaging failure in the published crate.

Acer Renoir Vulkan, second run, all 256 changed, median µs:

| Backend | CPU prepare | GPU raster |
|---|---:|---:|
| Glyphon | 202.88 | 255.00 |
| wgpu-font-renderer cached | 454.40 | 259.95 |
| Sluggrs Skylines | 887.25 | 995.76 |
| wgpu-font-renderer analytic | 156.39 | 1680.20 |
| Vello | 106.03 | 1837.26 (bracketed) |

Opaque-cell benchmark uses identical Glyphon coverage and two solid backgrounds.
All three output methods agree within two RGBA8 levels, including readback after
updates. Median GPU µs:

| Update | 1 | 8 | 64 | 256 |
|---|---:|---:|---:|---:|
| Restore background + alpha glyphs | 46.30 | 70.90 | 267.95 | 916.60 |
| Precolored opaque cells | 37.50 | 54.00 | 156.00 | 460.53 |
| Coverage mixed with background in shader | 46.85 | 42.35 | 156.95 | 460.90 |

Native-resolution composition: two layers 478.08 µs median, one opaque panel
212.88 µs. Full opaque redraws with clear were ~675 µs, versus ~519 µs for
the transparent fixed-cell layer. Cell tiles cover more pixels than the
tight glyph quads in the backend comparison. GPU load and clock variation can
affect timings. Prefer measuring changed glyph cells and narrow ink bounds
before selecting a production rendering path. No production backend change.

### Sparse native-resolution glyph overwrite

```sh
cargo test -p yarugc --locked --release --test text_backends -- compute_glyph_cells --ignored --nocapture
```

The same 256-slot fixture tests R8 inverse coverage (transmission = 1 − alpha)
and two 4-bit values packed into each R8Uint texel. The glyph masks originate in
Glyphon at the target physical resolution. Fixed nonoverlapping character cells
allow both compute storage writes and unblended instanced graphics writes using
the same update list. Each tight rectangle is the union of the previous and new
glyph ink bounds; all other pixels remain untouched. Both transition directions
are checked against the CPU reference for each variant and change count.

Acer Renoir Vulkan, median GPU microseconds, 48 samples per case:

| Method | 1 | 8 | 64 | 256 |
|---|---:|---:|---:|---:|
| Compute, full panel, R8 | 398.00 | 398.75 | 399.70 | 407.60 |
| Compute, changed full cells, R8 | 50.15 | 50.25 | 35.92 | 108.12 |
| Compute, tight R8, 64 lanes | 4.36 | 12.08 | 10.92 | 69.84 |
| Compute, tight R8, 32 lanes | 5.36 | 5.20 | 11.96 | 59.20 |
| Compute, tight R8, 128 lanes | 3.72 | 4.00 | 8.64 | 61.68 |
| Compute, tight packed R4, 64 lanes | 4.20 | 4.84 | 9.04 | 57.20 |
| Compute, tight R8, black text | 34.23 | 6.84 | 8.40 | 55.24 |
| Graphics, changed full cells, R8 | 5.72 | 8.08 | 27.96 | 100.12 |
| Graphics, tight R8 | 4.96 | 16.36 | 10.64 | 54.24 |

Full-change tight writes cover 150,025 pixels in 1,490 changed glyph cells,
versus 762,880 pixels for those full cells and 1,048,576 pixels for the whole
panel. Tight graphics p95 = 64.64 µs; 32-lane compute R8 p95 = 96.84 µs.
CPU list construction and staging upload take approximately 4–17 µs, excluding
text formatting, shaping, encoder creation and submission. GPU intervals
exclude staging uploads and final composition. Variability in GPU frequency and
load makes small differences (including workgroup sizes and black vs white)
inconclusive. Four-bit packing halves the 8 KiB test mask, with maximum channel
error 8/255 and whole-panel mean absolute channel error 0.1727/255; it is not
a compelling quality/performance tradeoff. Native R8 retains full coverage.
The tight bounds optimization benefits both graphics and compute; neither
requires destination reads or generic screen damage analysis. Static text stays
baked into the static window texture. This is an experimental update kernel,
not yet an integrated production backend.
