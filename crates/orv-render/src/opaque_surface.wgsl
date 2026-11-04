// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var output:texture_storage_2d<rgba16float,write>;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2i(vec2u(p.region.xy)+gid.xy);
    let sample=evaluate_surface(pixel);
    textureStore(output,pixel,vec4f(sample.x,sample.x,sample.x,1.));
}
