// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var output:texture_storage_2d<rgba16float,write>;
@group(0) @binding(4) var mask_cells:texture_2d<u32>;
@group(0) @binding(5) var mask_depth:texture_2d<f32>;
@group(0) @binding(6) var<storage,read_write> failed:atomic<u32>;
@group(0) @binding(7) var<uniform> mask:vec4f;

fn source_interval(origin:vec3f,direction:vec3f,offset:vec2f)->vec2f {
    let maximum=vec3f(textureDimensions(source))-1.;
    var near=-1.e30;var far=1.e30;
    for(var axis=0u;axis<3u;axis+=1u) {
        if direction[axis]==0. {
            if origin[axis]<0. || origin[axis]>maximum[axis] {return vec2f(1.,-1.);}
        } else {
            let a=-origin[axis]/direction[axis];let b=(maximum[axis]-origin[axis])/direction[axis];
            near=max(near,min(a,b));far=min(far,max(a,b));
        }
    }
    if p.clip_minimum.w>0.5 {
        let world=p.world_origin.xyz+p.world_right.xyz*offset.x+p.world_down.xyz*offset.y;
        for(var axis=0u;axis<3u;axis+=1u) {
            let delta=p.world_direction[axis];
            if delta==0. {
                if world[axis]<p.clip_minimum[axis] || world[axis]>p.clip_maximum[axis] {return vec2f(1.,-1.);}
            } else {
                let a=(p.clip_minimum[axis]-world[axis])/delta;let b=(p.clip_maximum[axis]-world[axis])/delta;
                near=max(near,min(a,b));far=min(far,max(a,b));
            }
        }
    }
    return vec2f(near,far);
}
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
