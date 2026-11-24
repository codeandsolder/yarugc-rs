struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) glyph: u32,
    @location(1) @interpolate(flat) bg: u32,
    @location(2) local: vec2<f32>,
}
@group(0) @binding(0) var tiles: texture_2d<f32>;
@group(0) @binding(1) var<storage, read> ids: array<u32>;

@vertex
fn tile_vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> Out {
    let corner = vec2<f32>(f32(v & 1u), f32(v >> 1u));
    let slot = i / 8u;
    let row = slot / 16u;
    let col = slot % 16u;
    let xy = vec2<f32>(f32(col * 128u + (i % 8u) * 16u), f32(row * 32u)) + corner * vec2(16.0, 32.0);
    return Out(vec4(xy / vec2(2048.0, 512.0) * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0), ids[i], (row+col)%2u, corner * vec2(16.0,32.0));
}

@fragment
fn alpha_fs(in: Out) -> @location(0) vec4<f32> {
    return textureLoad(tiles, vec2<i32>(i32(in.glyph*16u)+i32(in.local.x),i32(in.local.y)),0);
}
@fragment
fn opaque_fs(in: Out) -> @location(0) vec4<f32> {
    return textureLoad(tiles, vec2<i32>(i32(in.glyph*16u)+i32(in.local.x),i32(in.bg*32u)+i32(in.local.y)),0);
}
@fragment
fn background_fs(in: Out) -> @location(0) vec4<f32> {
    return select(vec4(16.,24.,32.,255.), vec4(24.,32.,40.,255.),in.bg==1u)/255.;
}

@vertex
fn full_vs(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u));
    return vec4(xy*vec2(2.0,-2.0)+vec2(-1.0,1.0),0.0,1.0);
}
@fragment
fn full_fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(tiles, vec2<i32>(p.xy),0);
}

@fragment
fn mixed_fs(in: Out) -> @location(0) vec4<f32> {
    let coverage = textureLoad(tiles, vec2<i32>(i32(in.glyph*16u)+i32(in.local.x),i32(in.local.y)),0).a;
    let bg = select(vec3(16.,24.,32.), vec3(24.,32.,40.),in.bg==1u)/255.;
    return vec4(mix(bg, vec3(1.0), coverage), 1.0);
}
