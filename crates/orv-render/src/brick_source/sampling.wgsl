// SPDX-License-Identifier: GPL-3.0-or-later
// Bind the exact page owner at group zero. Callers validate finite global source
// coordinates before dispatch. Nonresident lookup belongs to the page directory;
// no absence or unavailable stencil may be interpreted as transparent source.
struct BrickParameters {
    dimensions:vec4u, cells_minimum:vec4u,cells_maximum:vec4u,
    origin:vec4u,size:vec4u,
}
@group(0) @binding(0) var brick_values:texture_3d<f32>;
@group(0) @binding(1) var brick_validity:texture_3d<u32>;
@group(0) @binding(2) var<uniform> brick:BrickParameters;
const BRICK_KNOWN:u32=1u;
const BRICK_MISSING:u32=2u;
const BRICK_NOT_OWNED:u32=3u;
const BRICK_OUTSIDE_SOURCE:u32=4u;
const BRICK_STENCIL_UNAVAILABLE:u32=5u;
struct BrickValue {value:f32,status:u32}
struct BrickGradient {value:vec3f,status:u32}
fn brick_ownership(point:vec3f)->u32 {
    let maximum=vec3f(brick.dimensions.xyz)-1.;
    if any(point<vec3f(-0.000001)) || any(point>maximum+vec3f(0.000001)) {return BRICK_OUTSIDE_SOURCE;}
    let q=clamp(point,vec3f(0.),maximum);
    let cell=min(vec3u(floor(q)),brick.dimensions.xyz-2u);
    if any(cell<brick.cells_minimum.xyz) || any(cell>=brick.cells_maximum.xyz) {return BRICK_NOT_OWNED;}
    return BRICK_KNOWN;
}
fn brick_scalar_stencil(point:vec3f)->BrickValue {
    let maximum=brick.dimensions.xyz-1u;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    // Subtract the page origin only after computing the global integer stencil.
    // A floating-point local-coordinate subtraction would change its fraction.
    let ga=vec3u(floor(q));let gb=min(ga+1u,maximum);let t=fract(q);
    if any(ga<brick.origin.xyz) || any(gb>=brick.origin.xyz+brick.size.xyz) {return BrickValue(0.,BRICK_STENCIL_UNAVAILABLE);}
    let a=vec3i(ga-brick.origin.xyz);let b=vec3i(gb-brick.origin.xyz);
    if brick.dimensions.w!=0u {
        for(var z=0u;z<2u;z++) {for(var y=0u;y<2u;y++) {for(var x=0u;x<2u;x++) {
            let hi=vec3<bool>(x==1u,y==1u,z==1u);
            let w=select(vec3f(1.)-t,t,hi);
            if w.x*w.y*w.z>0. && textureLoad(brick_validity,select(a,b,hi),0).r==0u {return BrickValue(0.,BRICK_MISSING);}
        }}}
    }
    let c00=mix(textureLoad(brick_values,a,0).r,textureLoad(brick_values,vec3i(b.x,a.y,a.z),0).r,t.x);
    let c10=mix(textureLoad(brick_values,vec3i(a.x,b.y,a.z),0).r,textureLoad(brick_values,vec3i(b.x,b.y,a.z),0).r,t.x);
    let c01=mix(textureLoad(brick_values,vec3i(a.x,a.y,b.z),0).r,textureLoad(brick_values,vec3i(b.x,a.y,b.z),0).r,t.x);
    let c11=mix(textureLoad(brick_values,vec3i(a.x,b.y,b.z),0).r,textureLoad(brick_values,b,0).r,t.x);
    return BrickValue(mix(mix(c00,c10,t.y),mix(c01,c11,t.y),t.z),BRICK_KNOWN);
}
fn brick_scalar(point:vec3f)->BrickValue {
    let status=brick_ownership(point);
    if status!=BRICK_KNOWN {return BrickValue(0.,status);}
    return brick_scalar_stencil(point);
}
fn brick_index_gradient(point:vec3f)->BrickGradient {
    let value=brick_scalar(point);
    if value.status!=BRICK_KNOWN {return BrickGradient(vec3f(0.),value.status);}
    if brick.cells_minimum.w!=1u {return BrickGradient(vec3f(0.),BRICK_STENCIL_UNAVAILABLE);}
    let maximum=vec3f(brick.dimensions.xyz)-1.;let q=clamp(point,vec3f(0.),maximum);
    var gradient=vec3f(0.);
    for(var axis=0u;axis<3u;axis++) {
        var lo=q;var hi=q;
        lo[axis]=max(q[axis]-1.,0.);hi[axis]=min(q[axis]+1.,maximum[axis]);
        let a=brick_scalar_stencil(lo);let b=brick_scalar_stencil(hi);
        if a.status!=BRICK_KNOWN {return BrickGradient(vec3f(0.),a.status);}
        if b.status!=BRICK_KNOWN {return BrickGradient(vec3f(0.),b.status);}
        gradient[axis]=(b.value-a.value)/(hi[axis]-lo[axis]);
    }
    return BrickGradient(gradient,BRICK_KNOWN);
}
