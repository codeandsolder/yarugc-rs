// One workgroup per changed character (or a subrectangle of its cell).
// All destinations are disjoint; each invocation writes a single pixel.
struct Job {
    cell: u32,
    glyph: u32,
    // Two packed u16 pairs: local rectangle origin and extent.
    origin: u32,
    extent: u32,
}
@group(0) @binding(0) var mask8: texture_2d<f32>;
@group(0) @binding(1) var mask4: texture_2d<u32>;
@group(0) @binding(2) var<storage, read> jobs: array<Job>;
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm, write>;

// Black and white use the same expression with precomputed slope.
// White: 1 - (1-bg)*transmission; black: bg*transmission.
override packed: bool = false;
override white: bool = true;
override tight: bool = false;
override lanes: u32 = 64u;

fn transmission(p: vec2<u32>) -> f32 {
    if packed {
        let byte = textureLoad(mask4, vec2<i32>(i32(p.x / 2u), i32(p.y)), 0).r;
        return f32((byte >> ((p.x & 1u)*4u)) & 15u) / 15.0;
    }
    return textureLoad(mask8, vec2<i32>(p), 0).r;
}

@compute @workgroup_size(lanes)
fn write_cells(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let job = jobs[group.x];
    let slot = job.cell / 8u;
    let row = slot / 16u;
    let col = slot % 16u;
    let dest = vec2(col * 128u + (job.cell % 8u) * 16u, row * 32u);
    let bg = select(vec3(16.,24.,32.), vec3(24.,32.,40.), (row+col)%2u == 1u)/255.;
    let local_origin = select(vec2(0u), vec2(job.origin & 65535u, job.origin >> 16u), tight);
    let size = select(vec2(16u,32u), vec2(job.extent & 65535u, job.extent >> 16u), tight);
    let n = size.x * size.y;
    for (var i = lane; i < n; i += lanes) {
        let p = local_origin + vec2(i % size.x, i / size.x);
        let t = transmission(vec2(job.glyph * 16u + p.x, p.y));
        let color = select(bg*t, vec3(1.0)-(vec3(1.0)-bg)*t, white);
        textureStore(output, vec2<i32>(dest+p), vec4(color,1.0));
    }
}

struct GlyphVertex {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) glyph: u32,
    @location(1) @interpolate(flat) dest: vec2<u32>,
    @location(2) @interpolate(flat) bg: vec3<f32>,
}

@vertex
fn glyph_vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> GlyphVertex {
    let job = jobs[i];
    let slot = job.cell / 8u;
    let row = slot / 16u;
    let col = slot % 16u;
    let dest = vec2(col * 128u + (job.cell % 8u) * 16u, row * 32u);
    let origin = select(vec2(0u), vec2(job.origin & 65535u, job.origin >> 16u), tight);
    let extent = select(vec2(16u,32u), vec2(job.extent & 65535u, job.extent >> 16u), tight);
    let xy = vec2<f32>(dest + origin) + vec2(f32(v & 1u), f32(v >> 1u)) * vec2<f32>(extent);
    let bg = select(vec3(16.,24.,32.), vec3(24.,32.,40.), (row+col)%2u == 1u)/255.;
    return GlyphVertex(vec4(xy / vec2(2048.0,512.0) * vec2(2.0,-2.0) + vec2(-1.0,1.0),0.0,1.0), job.glyph, dest, bg);
}

@fragment
fn glyph_fs(in: GlyphVertex) -> @location(0) vec4<f32> {
    let p = vec2<u32>(in.pos.xy) - in.dest;
    let t = transmission(vec2(in.glyph * 16u + p.x, p.y));
    let color = select(in.bg*t, vec3(1.0)-(vec3(1.0)-in.bg)*t, white);
    return vec4(color,1.0);
}
