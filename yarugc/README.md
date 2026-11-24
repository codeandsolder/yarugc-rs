# YARUGC — Yet Another Rust GUI Crate

Engine-owned retained UI. Masonry handles widget state, layout and events. A cached
window renders to a persistent transparent texture only when its root requests a
redraw. Native GPU producers can update other surfaces independently. A prepared
composition reuses GPU placement buffers and texture bindings until geometry or
ordering changes. The host draws it over its own scene and presents.

This first slice deliberately does not perform general-purpose damage tracking.
Window-local input routing, docking, numeric widgets, dynamic semantic layer
extraction and batching beyond individual texture quads are subsequent work.
The compositor accepts straight or premultiplied RGBA8 layers in painter order.
Vello writes straight alpha; the compositor premultiplies before interpolation.
