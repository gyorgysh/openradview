// SPDX-License-Identifier: GPL-3.0-or-later
@group(1) @binding(0) var<storage,read_write> output:array<vec2u>;
@compute @workgroup_size(4,4,4)
fn main(@builtin(global_invocation_id) id:vec3u) {
    if any(id>=brick.size.xyz) {return;}
    let i=(id.z*brick.size.y+id.y)*brick.size.x+id.x;
    var known=1u;
    if brick.dimensions.w!=0u {known=textureLoad(brick_validity,vec3i(id),0).r;}
    output[i]=vec2u(bitcast<u32>(textureLoad(brick_values,vec3i(id),0).r),known);
}
