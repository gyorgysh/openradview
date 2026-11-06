// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    x:vec4f, y:vec4f, z:vec4f, depth:vec4f,
    world_x:vec4f, world_y:vec4f, world_z:vec4f,
    center:vec4f, clip_min:vec4f, clip_max:vec4f, control:vec4f,
    ray_origin:vec4f, ray_right:vec4f, ray_down:vec4f, ray_direction:vec4f,
};
@group(0) @binding(0) var<uniform> p:Params;
struct Vertex {
    @builtin(position) position:vec4f,
    @location(0) @interpolate(flat) cell:vec3u,
    @location(1) @interpolate(flat) a:vec3u,
    @location(2) @interpolate(flat) b:vec3u,
    @location(3) @interpolate(flat) c:vec3u,
};
@vertex fn vs(@location(0) cell:vec3u, @location(1) a:vec3u,
              @location(2) b:vec3u, @location(3) c:vec3u,
              @builtin(vertex_index) vertex:u32)->Vertex {
    let base=vec3f(cell)-p.center.xyz;
    let points=array<vec3u,3>(a,b,c);
    var minimum=vec2f(1e30); var maximum=vec2f(-1e30);
    for(var i=0u;i<3u;i++) {
        let h=vec4f(base+vec3f(points[i])/CANDIDATE_DENOMINATOR,1.);
        let xy=vec2f(dot(p.x,h),dot(p.y,h));
        minimum=min(minimum,xy); maximum=max(maximum,xy);
    }
    // Guard against fixed-point raster coverage; the fragment checks the actual triangle.
    let guard=2.*vec2f(p.ray_origin.w,p.ray_right.w);
    minimum-=guard; maximum+=guard;
    let rectangle=array<vec2f,6>(vec2f(0.,0.),vec2f(1.,0.),vec2f(1.,1.),vec2f(0.,0.),vec2f(1.,1.),vec2f(0.,1.));
    var out:Vertex; out.position=vec4f(mix(minimum,maximum,rectangle[vertex]),.5,1.);
    out.cell=cell; out.a=a; out.b=b; out.c=c; return out;
}
struct Pixel { @location(0) cell:vec4u, @location(1) depth:f32, @builtin(frag_depth) hardware_depth:f32 };
@fragment fn fs(input:Vertex)->Pixel {
    let uv=pixel_ray_offset(vec2i(input.position.xy),vec2u(vec2f(p.ray_down.w,p.ray_direction.w)));
    let base=vec3f(input.cell)-p.center.xyz;
    let origin=p.ray_origin.xyz+p.ray_right.xyz*uv.x+p.ray_down.xyz*uv.y-base;
    let direction=p.ray_direction.xyz;
    let a=vec3f(input.a)/CANDIDATE_DENOMINATOR;
    let b=vec3f(input.b)/CANDIDATE_DENOMINATOR;
    let c=vec3f(input.c)/CANDIDATE_DENOMINATOR;
    let e1=b-a; let e2=c-a; let cross_direction=cross(direction,e2);
    let determinant=dot(e1,cross_direction);
    if determinant==0. { discard; }
    let delta=origin-a; let u=dot(delta,cross_direction)/determinant;
    let cross_delta=cross(delta,e1); let v=dot(direction,cross_delta)/determinant;
    if !(u>=0. && v>=0. && u+v<=1.) { discard; }
    // Reconstruct on the triangle instead of subtracting a long ray distance.
    let h=vec4f(base+a+u*e1+v*e2,1.);
    let world=vec3f(dot(p.world_x,h),dot(p.world_y,h),dot(p.world_z,h));
    let hardware_depth=dot(p.z,h);
    let depth=dot(p.depth,h);
    if !(hardware_depth>=0. && hardware_depth<=1.) || !(abs(depth)<=1e20) || !all(abs(world)<=vec3f(1e20)) { discard; }
    if p.control.y>0.5 && (any(world<p.clip_min.xyz)||any(world>p.clip_max.xyz)) { discard; }
    var out:Pixel; out.cell=vec4u(input.cell,u32(p.control.x));
    out.depth=depth; out.hardware_depth=hardware_depth; return out;
}
