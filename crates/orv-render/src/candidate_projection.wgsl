// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    x:vec4f, y:vec4f, z:vec4f, depth:vec4f,
    world_x:vec4f, world_y:vec4f, world_z:vec4f,
    center:vec4f, clip_min:vec4f, clip_max:vec4f, control:vec4f,
    ray_origin:vec4f, ray_right:vec4f, ray_down:vec4f, ray_direction:vec4f,
    gradient_x:vec4f, gradient_y:vec4f, gradient_z:vec4f,
};
@group(0) @binding(0) var<uniform> p:Params;
struct Vertex {
    @builtin(position) position:vec4f,
    @location(0) @interpolate(flat) cell:vec3u,
    @location(1) @interpolate(flat) a:vec3u,
    @location(2) @interpolate(flat) b:vec3u,
    @location(3) @interpolate(flat) c:vec3u,
    @location(4) @interpolate(flat) samples:vec4u,
};
@vertex fn vs(@location(0) cell:vec3u, @location(1) a:vec3u,
              @location(2) b:vec3u, @location(3) c:vec3u,
              @location(4) samples:vec4u,
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
    out.cell=cell; out.a=a; out.b=b; out.c=c; out.samples=samples; return out;
}
fn signed_sample(bits:u32)->f32 {
    let value=i32(bits&65535u);
    return f32(select(value,value-65536,(bits&32768u)!=0u));
}
fn field_gradient(samples:vec4u, point:vec3f)->vec3f {
    let a=signed_sample(samples.x); let b=signed_sample(samples.x>>16u);
    let c=signed_sample(samples.y); let d=signed_sample(samples.y>>16u);
    let e=signed_sample(samples.z); let f=signed_sample(samples.z>>16u);
    let g=signed_sample(samples.w); let h=signed_sample(samples.w>>16u);
    let dx=mix(mix(b-a,d-c,point.y),mix(f-e,h-g,point.y),point.z);
    let dy=mix(mix(c-a,d-b,point.x),mix(g-e,h-f,point.x),point.z);
    let dz=mix(mix(e-a,f-b,point.x),mix(g-c,h-d,point.x),point.y);
    return vec3f(dx,dy,dz);
}
struct Pixel { @location(0) cell:vec4u, @location(1) depth:f32, @location(2) normal:vec4f, @builtin(frag_depth) hardware_depth:f32 };
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
    let point=clamp(a+u*e1+v*e2,vec3f(0.),vec3f(1.));
    let h=vec4f(base+point,1.);
    let world=vec3f(dot(p.world_x,h),dot(p.world_y,h),dot(p.world_z,h));
    let hardware_depth=dot(p.z,h);
    let depth=dot(p.depth,h);
    if !(hardware_depth>=0. && hardware_depth<=1.) || !(abs(depth)<=1e20) || !all(abs(world)<=vec3f(1e20)) { discard; }
    if p.control.y>0.5 && (any(world<p.clip_min.xyz)||any(world>p.clip_max.xyz)) { discard; }
    let gradient=field_gradient(input.samples,point);
    let physical=vec3f(dot(p.gradient_x.xyz,gradient),dot(p.gradient_y.xyz,gradient),dot(p.gradient_z.xyz,gradient));
    let scale=max(abs(physical.x),max(abs(physical.y),abs(physical.z)));
    var normal=vec4f(0.);
    if scale>0. && scale<=1e30 {
        let unit=physical/scale;
        let magnitude=length(unit);
        if magnitude>0. && magnitude<=2. { normal=vec4f(-unit/magnitude,1.); }
    }
    var out:Pixel; out.cell=vec4u(input.cell,u32(p.control.x)); out.normal=normal;
    out.depth=depth; out.hardware_depth=hardware_depth; return out;
}
