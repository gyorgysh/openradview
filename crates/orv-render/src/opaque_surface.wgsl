// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var output:texture_storage_2d<rgba16float,write>;
@group(0) @binding(4) var surface_cells:texture_2d<u32>;
@group(0) @binding(5) var surface_depth:texture_2d<f32>;
@group(0) @binding(6) var<storage,read_write> failed:atomic<u32>;
// Linear base luminance, u16 ID, two-sided ambient and diffuse weights.
@group(0) @binding(7) var<uniform> surface:vec4f;
fn refuse(pixel:vec2i) {
    atomicStore(&failed,1u);textureStore(output,pixel,vec4f(0.));
}
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2i(vec2u(p.region.xy)+gid.xy);
    let cell=textureLoad(surface_cells,pixel,0);
    let depth=textureLoad(surface_depth,pixel,0).r;
    let hit=cell.w!=0u;
    let offset=pixel_ray_offset(pixel,textureDimensions(output));
    let origin=p.origin.xyz+p.right.xyz*offset.x+p.down.xyz*offset.y;
    var ray=source_interval(origin,p.direction.xyz,offset);
    var background=0.0030352698;
    if hit {
        let side=(cell.w>>16u)&7u;let axis=side/2u;
        if (cell.w&65535u)!=u32(surface.y) || side>5u || (cell.w&~0x0107ffffu)!=0u ||
            any(cell.xyz>=textureDimensions(source)) || !(depth>=-1e20 && depth<=1e20) {
            refuse(pixel);return;
        }
        let sign=select(-1.,1.,(side&1u)!=0u);
        let local=origin+p.direction.xyz*depth-vec3f(cell.xyz);
        // This validates arithmetic consistency, not exact membership or pick
        // authority. Coincident edge contacts are still a raster/f32 choice.
        if !all(abs(local)<=vec3f(.505)) || abs(local[axis]-sign*.5)>.005 {
            refuse(pixel);return;
        }
        var normal=vec3f(p.inverse_x[axis],p.inverse_y[axis],p.inverse_z[axis])*sign;
        let scale=max(max(abs(normal.x),abs(normal.y)),abs(normal.z));
        if !(scale>0. && scale<=1e20) {refuse(pixel);return;}
        normal=normalize(normal/scale);
        let light=surface.z+surface.w*abs(dot(normal,p.light.xyz));
        background=surface.x*light;
        ray.y=min(ray.y,depth);
    } else if any(cell.xyz!=vec3u(0u)) || depth!=0. {
        refuse(pixel);return;
    }
    // Known open interfaces may expose a real backface. Source in front still
    // attenuates it; source behind the opaque face contributes no radiance.
    let integrated=integrate_surface_source(origin,ray);
    let result=integrated.x+integrated.y*background;
    if !(result>=0. && result<=1.00001) {refuse(pixel);return;}
    textureStore(output,pixel,vec4f(result,result,result,1.));
}
