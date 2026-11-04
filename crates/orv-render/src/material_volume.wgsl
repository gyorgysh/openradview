// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var output:texture_storage_2d<rgba16float,write>;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2u(p.region.xy)+gid.xy;
    let uv=(vec2f(pixel)+0.5)/vec2f(textureDimensions(output));
    let origin=p.origin.xyz+p.right.xyz*(uv.x-0.5)+p.down.xyz*(uv.y-0.5);
    let ray=clipped_interval(uv,interval(origin,p.direction.xyz));
    let integrated=integrate_material(origin,ray);
    let radiance=integrated.x;let transmittance=integrated.y;
    let result=radiance+transmittance*0.0030352698;
    textureStore(output,vec2i(pixel),vec4f(result,result,result,1.));
}
