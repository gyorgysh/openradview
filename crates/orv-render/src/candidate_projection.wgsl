// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    x:vec4f, y:vec4f, z:vec4f, depth:vec4f,
    world_x:vec4f, world_y:vec4f, world_z:vec4f,
    center:vec4f, clip_min:vec4f, clip_max:vec4f, control:vec4f,
    ray_origin:vec4f, ray_right:vec4f, ray_down:vec4f, ray_direction:vec4f,
    gradient_x:vec4f, gradient_y:vec4f, gradient_z:vec4f,
};
@group(0) @binding(0) var<uniform> p:Params;
// Carry exact cell-boundary coordinates before conversion, so either owner of
// a shared grid vertex reconstructs the same source-centred position.
fn centered_point(cell:vec3u, local:vec3u)->vec3f {
    let grid=u32(CANDIDATE_DENOMINATOR);
    let whole=local/vec3u(grid);
    let fraction=local%vec3u(grid);
    return vec3f(cell+whole)-p.center.xyz+vec3f(fraction)/CANDIDATE_DENOMINATOR;
}
// Evaluate both owners' shared edge in the same endpoint order, including on
// hardware that contracts multiplication and subtraction differently.
fn edge_less(a:vec3u,b:vec3u)->bool {
    return a.x<b.x || (a.x==b.x && (a.y<b.y || (a.y==b.y && a.z<b.z)));
}
fn oriented_edge(a:vec2f,b:vec2f,ka:vec3u,kb:vec3u)->f32 {
    if edge_less(ka,kb) { return a.x*b.y-a.y*b.x; }
    return -(b.x*a.y-b.y*a.x);
}
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
    let points=array<vec3u,3>(a,b,c);
    var minimum=vec2f(1e30); var maximum=vec2f(-1e30);
    for(var i=0u;i<3u;i++) {
        let h=vec4f(centered_point(cell,points[i]),1.);
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
    let origin=p.ray_origin.xyz+p.ray_right.xyz*uv.x+p.ray_down.xyz*uv.y;
    let direction=p.ray_direction.xyz;
    let a=centered_point(input.cell,input.a)-origin;
    let b=centered_point(input.cell,input.b)-origin;
    let c=centered_point(input.cell,input.c)-origin;
    // Project along the dominant ray axis. Both triangles at a shared edge
    // then evaluate that edge from the same two centred endpoints.
    let ad=abs(direction);
    var kz=0u;
    if ad.y>ad.x { kz=1u; }
    if ad.z>ad[kz] { kz=2u; }
    if direction[kz]==0. { discard; }
    var kx=(kz+1u)%3u; var ky=(kx+1u)%3u;
    if direction[kz]<0. { let swap=kx; kx=ky; ky=swap; }
    let shear=vec2f(direction[kx],direction[ky])/direction[kz];
    let pa=vec2f(a[kx],a[ky])-shear*a[kz];
    let pb=vec2f(b[kx],b[ky])-shear*b[kz];
    let pc=vec2f(c[kx],c[ky])-shear*c[kz];
    let weights=vec3f(oriented_edge(pb,pc,input.b,input.c),
                      oriented_edge(pc,pa,input.c,input.a),
                      oriented_edge(pa,pb,input.a,input.b));
    if any(weights<vec3f(0.)) && any(weights>vec3f(0.)) { discard; }
    let determinant=weights.x+weights.y+weights.z;
    if determinant==0. { discard; }
    let bary=weights/determinant;
    // Reconstruct in cell-local coordinates to retain local gradient precision.
    let point=clamp((vec3f(input.a)*bary.x+vec3f(input.b)*bary.y+vec3f(input.c)*bary.z)/CANDIDATE_DENOMINATOR,vec3f(0.),vec3f(1.));
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
