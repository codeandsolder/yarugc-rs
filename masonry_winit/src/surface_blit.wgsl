// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

@group(0) @binding(0) var source: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let xy = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(xy[vertex], 0.0, 1.0);
}

fn pixel(position: vec4<f32>) -> vec4<f32> {
    return textureLoad(source, vec2<i32>(position.xy), 0);
}

@fragment
fn copy(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return pixel(position);
}

@fragment
fn premultiply(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let c = pixel(position);
    return vec4(c.rgb * c.a, c.a);
}

@fragment
fn unpremultiply(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let c = pixel(position);
    if c.a == 0.0 {
        return vec4(0.0);
    }
    return vec4(c.rgb / c.a, c.a);
}
