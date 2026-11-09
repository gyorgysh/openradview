// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(4) var plane0:texture_2d<f32>;
@group(0) @binding(5) var plane1:texture_2d<f32>;
@group(0) @binding(6) var plane2:texture_2d<f32>;
@group(0) @binding(7) var voitable:texture_2d_array<f32>;
@group(0) @binding(8) var<storage,read_write> failed:atomic<u32>;
struct Section {origin:vec4f,right:vec4f,down:vec4f,direction:vec4f,window:vec4f,voi:vec4f,clip:array<vec4f,6>}
@group(0) @binding(9) var<uniform> sections:array<Section,3>;
@group(0) @binding(10) var surface_cells:texture_2d<u32>;
@group(0) @binding(11) var surface_depth:texture_2d<f32>;
struct Boundary {material:vec4f,flags:vec4f}
@group(0) @binding(12) var<uniform> boundary:Boundary;
fn surface_background(pixel:vec2i,origin:vec3f)->vec2f {
    let empty=vec2f(1e30,0.0030352698);
    if boundary.flags.x<.5 {return empty;}
    let cell=textureLoad(surface_cells,pixel,0);let depth=textureLoad(surface_depth,pixel,0).r;
    if cell.w==0u {
        if any(cell.xyz!=vec3u(0u)) || depth!=0. {atomicStore(&failed,1u);}
        return empty;
    }
    let side=(cell.w>>16u)&7u;let axis=side/2u;
    if (cell.w&65535u)!=u32(boundary.material.y) || side>5u || (cell.w&~0x0107ffffu)!=0u ||
        any(cell.xyz>=textureDimensions(source)) || !(depth>=-1e20 && depth<=1e20) {
        atomicStore(&failed,1u);return empty;
    }
    let sign=select(-1.,1.,(side&1u)!=0u);
    let local=origin+p.direction.xyz*depth-vec3f(cell.xyz);
    if !all(abs(local)<=vec3f(.505)) || abs(local[axis]-sign*.5)>.005 {atomicStore(&failed,1u);return empty;}
    var normal=vec3f(p.inverse_x[axis],p.inverse_y[axis],p.inverse_z[axis])*sign;
    let scale=max(max(abs(normal.x),abs(normal.y)),abs(normal.z));
    if !(scale>0. && scale<=1e20) {atomicStore(&failed,1u);return empty;}
    normal=normalize(normal/scale);
    let light=boundary.material.z+boundary.material.w*abs(dot(normal,p.light.xyz));
    return vec2f(depth,boundary.material.x*light);
}
fn plane_texel(slot:u32,xy:vec2i)->vec2f {
    if slot==0u {return textureLoad(plane0,xy,0).rg;}
    if slot==1u {return textureLoad(plane1,xy,0).rg;}
    return textureLoad(plane2,xy,0).rg;
}
fn section_sample(slot:u32,xy:vec2f,size:vec2i)->vec2f {
    // Match scalar slice presentation: exact zero-weight missing neighbours do
    // not erase a valid node. A positive-weight missing neighbour stays unknown.
    let nearest=round(xy);
    let q=select(xy,nearest,abs(xy-nearest)<vec2f(0.00001));
    let base=vec2i(floor(q));let blend=fract(q);
    let offsets=array<vec2i,4>(vec2i(0),vec2i(1,0),vec2i(0,1),vec2i(1));
    let weights=array<f32,4>((1.-blend.x)*(1.-blend.y),blend.x*(1.-blend.y),(1.-blend.x)*blend.y,blend.x*blend.y);
    var value=0.;
    for(var i=0u;i<4u;i++) {
        if weights[i]<=0. {continue;}
        let sample=plane_texel(slot,clamp(base+offsets[i],vec2i(0),size-1));
        if sample.y<.5 {return vec2f(0.);}
        value+=sample.x*weights[i];
    }
    return vec2f(value,1.);
}
fn plane_luminance(slot:u32,value:f32)->f32 {
    let s=sections[slot];
    var gray=analytic_voi(value,s.window.x,s.window.y,s.voi.x);
    if s.voi.x>2.5 {
        let index=i32(clamp(floor(value-s.voi.y),0.,s.voi.z-1.));
        gray=textureLoad(voitable,vec2i(index%256,index/256),i32(s.voi.w),0).r;
    }
    if s.window.z>.5 {gray=1.-gray;}
    return select(gray/12.92,pow((gray+.055)/1.055,2.4),gray>.04045);
}
fn plane_inside_clip(s:Section,xy:vec2f)->bool {
    for(var i=0u;i<6u;i++) {
        let halfspace=s.clip[i];
        if dot(halfspace.xyz,vec3f(xy,1.))<0. {return false;}
    }
    return true;
}
// Display and a bounded local probe must use identical actor ordering and optics.
// x=radiance, y=foreground transmission, z=actor depth, w=0 background / 1 face /
// 2..4 source-plane slot. This grants no source-membership authority by itself.
fn evaluate_section(pixel:vec2i)->vec4f {
    // Region describes a batch, while ray centring always uses the whole image.
    let size=vec2u(vec2f(p.origin.w,p.right.w));let offset=pixel_ray_offset(pixel,size);
    let origin=p.origin.xyz+p.right.xyz*offset.x+p.down.xyz*offset.y;
    var ray=source_interval(origin,p.direction.xyz,offset);
    let surface=surface_background(pixel,origin);
    var nearest=surface.x;var background=surface.y;
    var actor=select(0.,1.,surface.x<1e30);
    for(var slot=0u;slot<3u;slot++) {
        let s=sections[slot];if s.down.w<.5 || s.direction.z==0. {continue;}
        let local=s.origin.xyz+s.right.xyz*offset.x+s.down.xyz*offset.y;
        let depth=-local.z/s.direction.z;
        if !(depth>=-1e20 && depth<=1e20) {atomicStore(&failed,1u);continue;}
        if depth>=nearest {continue;}
        let xy=local.xy+s.direction.xy*depth;let dimensions=vec2i(vec2f(s.origin.w,s.right.w));
        if !all(abs(xy)<=vec2f(1e20)) {atomicStore(&failed,1u);continue;}
        if any(xy<vec2f(-.5)) || any(xy>=vec2f(dimensions)-.5) {continue;}
        if !plane_inside_clip(s,xy) {continue;}
        let sample=section_sample(slot,xy,dimensions);if sample.y<.5 {continue;}
        nearest=depth;background=plane_luminance(slot,sample.x);actor=f32(slot+2u);
    }
    ray.y=min(ray.y,nearest);
    let integrated=integrate_surface_source(origin,ray);
    let result=integrated.x+integrated.y*background;
    if !(result>=0. && result<=1.00001) {atomicStore(&failed,1u);}
    return vec4f(result,integrated.y,nearest,actor);
}
