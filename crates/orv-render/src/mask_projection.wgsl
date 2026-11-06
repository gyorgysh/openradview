// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    origin:vec4f,right:vec4f,down:vec4f,direction:vec4f,
    world_origin:vec4f,world_right:vec4f,world_down:vec4f,world_direction:vec4f,
    clip_minimum:vec4f,clip_maximum:vec4f,region:vec4f,control:vec4f
}
@group(0) @binding(1) var validity:texture_3d<u32>;
@group(0) @binding(2) var<uniform> p:Params;
@group(0) @binding(3) var cells:texture_storage_2d<rgba32uint,write>;
@group(0) @binding(4) var depths:texture_storage_2d<r32float,write>;
const ERROR:u32=0xffffffffu;
// Neighboring cells share one integer half-grid face before conversion to f32.
fn face_time(face:i32,origin:f32,direction:f32)->f32 {
    return (f32(face)*0.5-origin)/direction;
}
fn store(pixel:vec2i,cell:vec3i,id:u32,distance:f32) {
    textureStore(cells,pixel,vec4u(vec3u(max(cell,vec3i(0))),id));
    textureStore(depths,pixel,vec4f(distance,0.,0.,1.));
}
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2i(vec2u(p.region.xy)+gid.xy);
    // Subtract the centre before division: an exact centre ray must not
    // acquire a signed offset from a backend's reciprocal approximation.
    let offset=pixel_ray_offset(pixel,textureDimensions(cells));
    let origin=p.origin.xyz+p.right.xyz*offset.x+p.down.xyz*offset.y;
    let direction=p.direction.xyz;
    let dimensions=mask_dimensions();
    var near=-1.e30;var far=1.e30;
    for(var axis=0u;axis<3u;axis+=1u) {
        let minimum=-0.5;let maximum=f32(dimensions[axis])-0.5;
        if direction[axis]==0. {
            if origin[axis]<minimum || origin[axis]>=maximum {store(pixel,vec3i(0),0u,0.);return;}
        } else {
            let a=face_time(-1,origin[axis],direction[axis]);let b=face_time(2*dimensions[axis]-1,origin[axis],direction[axis]);
            near=max(near,min(a,b));far=min(far,max(a,b));
        }
    }
    if p.control.z>0.5 {
        let world=p.world_origin.xyz+p.world_right.xyz*offset.x+p.world_down.xyz*offset.y;
        for(var axis=0u;axis<3u;axis+=1u) {
            let delta=p.world_direction[axis];
            if delta==0. {
                if world[axis]<p.clip_minimum[axis] || world[axis]>p.clip_maximum[axis] {store(pixel,vec3i(0),0u,0.);return;}
            } else {
                let a=(p.clip_minimum[axis]-world[axis])/delta;let b=(p.clip_maximum[axis]-world[axis])/delta;
                near=max(near,min(a,b));far=min(far,max(a,b));
            }
        }
    }
    if abs(near)>=1.e29 || abs(far)>=1.e29 {store(pixel,vec3i(0),ERROR,0.);return;}
    if near>=far {store(pixel,vec3i(0),0u,0.);return;}
    var cell=vec3i(clamp(floor(fma(direction,vec3f(near),origin)+0.5),vec3f(0.),vec3f(dimensions-1)));
    let step=vec3i(sign(direction));
    var crossing=vec3f(1.e30);
    for(var axis=0u;axis<3u;axis+=1u) {
        if direction[axis]==0. {continue;}
        for(var adjust=0u;adjust<3u;adjust+=1u) {
            let a=face_time(2*cell[axis]-1,origin[axis],direction[axis]);let b=face_time(2*cell[axis]+1,origin[axis],direction[axis]);
            if near<min(a,b) {cell[axis]-=step[axis];}
            else if near>=max(a,b) {cell[axis]+=step[axis];}
            else {break;}
        }
        let a=face_time(2*cell[axis]-1,origin[axis],direction[axis]);let b=face_time(2*cell[axis]+1,origin[axis],direction[axis]);
        if near<min(a,b) || near>=max(a,b) {store(pixel,vec3i(0),ERROR,0.);return;}
        // Retain the validated exit. Recomputing it with a signed half-step
        // can round differently at a clipped face on a fast-math backend.
        crossing[axis]=max(a,b);
    }
    var cursor=near;let limit=u32(dimensions.x+dimensions.y+dimensions.z)+3u;
    for(var visited=0u;visited<limit;visited+=1u) {
        if any(cell<vec3i(0)) || any(cell>=dimensions) {store(pixel,vec3i(0),ERROR,0.);return;}
        let next=min(far,min(crossing.x,min(crossing.y,crossing.z)));
        if next<=cursor {store(pixel,vec3i(0),ERROR,0.);return;}
        if mask_member(cell) {
            if source_eligible(cell) {store(pixel,cell,u32(p.control.x),cursor);return;}
        }
        cursor=next;
        if cursor>=far {store(pixel,vec3i(0),0u,0.);return;}
        // A corner has no positive-length side cells. Advance every tied plane.
        for(var axis=0u;axis<3u;axis+=1u) {
            if crossing[axis]<=next {
                cell[axis]+=step[axis];
                crossing[axis]=face_time(2*cell[axis]+step[axis],origin[axis],direction[axis]);
            }
        }
    }
    store(pixel,vec3i(0),ERROR,0.);
}
