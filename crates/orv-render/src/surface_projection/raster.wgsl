// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    x:vec4f, y:vec4f, z:vec4f, depth:vec4f,
    world_x:vec4f, world_y:vec4f, world_z:vec4f,
    normals:array<vec4f,6>, center:vec4f, clip_min:vec4f, clip_max:vec4f, control:vec4f,
    ray_origin:vec4f, ray_right:vec4f, ray_down:vec4f, ray_direction:vec4f,
};
@group(0) @binding(0) var<uniform> p:Params;
struct Vertex {
    @builtin(position) position:vec4f,
    @location(2) @interpolate(flat) cell_face:vec4u,
    @location(3) @interpolate(flat) normal:vec3f,
};
@vertex fn vs(@location(0) descriptor:vec4u,@builtin(vertex_index) vertex:u32)->Vertex {
    let side=descriptor.w&7u;let axis=side/2u;let u=(axis+1u)%3u;let v=(axis+2u)%3u;
    let corners=array<vec2f,4>(vec2f(-.5,-.5),vec2f(.5,-.5),vec2f(.5,.5),vec2f(-.5,.5));
    var minimum=vec2f(1e30);var maximum=vec2f(-1e30);
    for(var i=0u;i<4u;i++) {
        var q=vec3f(descriptor.xyz)-p.center.xyz;
        q[axis]+=select(-.5,.5,(side&1u)!=0u);q[u]+=corners[i].x;q[v]+=corners[i].y;
        let h=vec4f(q,1.);let xy=vec2f(dot(p.x,h),dot(p.y,h));
        minimum=min(minimum,xy);maximum=max(maximum,xy);
    }
    // Fixed-point triangle coverage must not omit a physical quad near an edge.
    // A one-pixel guard only expands candidate coverage; the fragment intersects
    // and bounds the actual source-cell face before writing any output.
    let guard=2.*vec2f(p.ray_origin.w,p.ray_right.w);
    minimum-=guard;maximum+=guard;
    let rectangle=array<vec2f,6>(vec2f(0.,0.),vec2f(1.,0.),vec2f(1.,1.),vec2f(0.,0.),vec2f(1.,1.),vec2f(0.,1.));
    var out:Vertex;out.position=vec4f(mix(minimum,maximum,rectangle[vertex]),.5,1.);
    out.cell_face=descriptor;out.normal=p.normals[side].xyz;
    return out;
}
struct Pixel { @location(0) cell:vec4u, @location(1) depth:f32, @location(2) normal:vec4f, @builtin(frag_depth) hardware_depth:f32 };
@fragment fn fs(input:Vertex)->Pixel {
    let flags=input.cell_face.w;let side=flags&7u;let axis=side/2u;
    let uv=pixel_ray_offset(vec2i(input.position.xy),vec2u(vec2f(p.ray_down.w,p.ray_direction.w)));
    let origin=p.ray_origin.xyz+p.ray_right.xyz*uv.x+p.ray_down.xyz*uv.y;
    if p.ray_direction[axis]==0. {discard;}
    let plane=f32(input.cell_face[axis])-p.center[axis]+select(-.5,.5,(side&1u)!=0u);
    let t=(plane-origin[axis])/p.ray_direction[axis];
    var q=origin+p.ray_direction.xyz*t;q[axis]=plane;
    let local=q+p.center.xyz-vec3f(input.cell_face.xyz);
    if !all(abs(local)<=vec3f(.5)) {discard;}
    let h=vec4f(q,1.);
    let world=vec3f(dot(p.world_x,h),dot(p.world_y,h),dot(p.world_z,h));
    if p.control.y>0.5 && (any(world<p.clip_min.xyz)||any(world>p.clip_max.xyz)) {discard;}
    var out:Pixel;
    out.cell=vec4u(input.cell_face.xyz,u32(p.control.x)|((flags&7u)<<16u)|((flags&256u)<<16u));
    out.depth=dot(p.depth,h);out.normal=vec4f(input.normal,1.);out.hardware_depth=dot(p.z,h);return out;
}
