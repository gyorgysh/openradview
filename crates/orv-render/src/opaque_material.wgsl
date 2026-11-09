// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var output:texture_storage_2d<rgba16float,write>;
@group(0) @binding(4) var mask_cells:texture_2d<u32>;
@group(0) @binding(5) var mask_depth:texture_2d<f32>;
@group(0) @binding(6) var<storage,read_write> failed:atomic<u32>;
@group(0) @binding(7) var<uniform> mask:vec4f;

@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2i(vec2u(p.region.xy)+gid.xy);
    let cell=textureLoad(mask_cells,pixel,0);
    if cell.w!=0u && cell.w!=u32(mask.y) {
        atomicStore(&failed,1u);textureStore(output,pixel,vec4f(0.));return;
    }
    let hit=cell.w!=0u;
    let offset=pixel_ray_offset(pixel,textureDimensions(output));
    let origin=p.origin.xyz+p.right.xyz*offset.x+p.down.xyz*offset.y;
    var ray=source_interval(origin,p.direction.xyz,offset);
    if hit {ray.y=min(ray.y,textureLoad(mask_depth,pixel,0).r);}
    // The mask may start outside the scalar-centre interpolation domain. That
    // half-cell space is empty source, rather than a clamped scalar extension.
    let integrated=integrate_material(origin,ray);
    let background=select(0.0030352698,mask.x,hit);
    let result=integrated.x+integrated.y*background;
    textureStore(output,pixel,vec4f(result,result,result,1.));
}
