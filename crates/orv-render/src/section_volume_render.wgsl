// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var output:texture_storage_2d<rgba16float,write>;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2i(vec2u(p.region.xy)+gid.xy);
    let value=evaluate_section(pixel).x;
    textureStore(output,pixel,vec4f(value,value,value,1.));
}
