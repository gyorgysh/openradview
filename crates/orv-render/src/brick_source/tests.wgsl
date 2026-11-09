// SPDX-License-Identifier: GPL-3.0-or-later
@group(1) @binding(0) var<storage,read> points:array<vec4f>;
@group(1) @binding(1) var<storage,read_write> output:array<u32>;
@group(1) @binding(2) var<storage,read> reference_values:array<f32>;
@group(1) @binding(3) var<storage,read> reference_validity:array<u32>;
fn ref_index(p:vec3u)->u32 {return (p.z*REF_DIM.y+p.y)*REF_DIM.x+p.x;}
fn ref_value(point:vec3f)->BrickValue {
    let maximum=vec3f(REF_DIM)-1.;
    if any(point<vec3f(-0.000001)) || any(point>maximum+vec3f(0.000001)) {return BrickValue(0.,BRICK_OUTSIDE_SOURCE);}
    let q=clamp(point,vec3f(0.),maximum);let a=vec3u(floor(q));let b=min(a+1u,REF_DIM-1u);let t=fract(q);
    for(var z=0u;z<2u;z++) {for(var y=0u;y<2u;y++) {for(var x=0u;x<2u;x++) {
        let hi=vec3<bool>(x==1u,y==1u,z==1u);let w=select(vec3f(1.)-t,t,hi);
        if w.x*w.y*w.z>0. && reference_validity[ref_index(select(a,b,hi))]==0u {return BrickValue(0.,BRICK_MISSING);}
    }}}
    let c00=scalar_mix(reference_values[ref_index(a)],reference_values[ref_index(vec3u(b.x,a.y,a.z))],t.x);
    let c10=scalar_mix(reference_values[ref_index(vec3u(a.x,b.y,a.z))],reference_values[ref_index(vec3u(b.x,b.y,a.z))],t.x);
    let c01=scalar_mix(reference_values[ref_index(vec3u(a.x,a.y,b.z))],reference_values[ref_index(vec3u(b.x,a.y,b.z))],t.x);
    let c11=scalar_mix(reference_values[ref_index(vec3u(a.x,b.y,b.z))],reference_values[ref_index(b)],t.x);
    return BrickValue(scalar_mix(scalar_mix(c00,c10,t.y),scalar_mix(c01,c11,t.y),t.z),BRICK_KNOWN);
}
fn ref_gradient(point:vec3f)->BrickGradient {
    let centre=ref_value(point);if centre.status!=BRICK_KNOWN {return BrickGradient(vec3f(0.),centre.status);}
    let maximum=vec3f(REF_DIM)-1.;let q=clamp(point,vec3f(0.),maximum);var gradient=vec3f(0.);
    for(var axis=0u;axis<3u;axis++) {var lo=q;var hi=q;lo[axis]=max(q[axis]-1.,0.);hi[axis]=min(q[axis]+1.,maximum[axis]);
        let a=ref_value(lo);let b=ref_value(hi);
        if a.status!=BRICK_KNOWN {return BrickGradient(vec3f(0.),a.status);}if b.status!=BRICK_KNOWN {return BrickGradient(vec3f(0.),b.status);}
        gradient[axis]=(b.value-a.value)/(hi[axis]-lo[axis]);
    }return BrickGradient(gradient,BRICK_KNOWN);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id:vec3u) {
    if id.x>=arrayLength(&points) {return;}
    let point=points[id.x].xyz;let a=brick_scalar(point);let g=brick_index_gradient(point);let b=ref_value(point);let h=ref_gradient(point);let i=id.x*12u;
    output[i]=bitcast<u32>(a.value);output[i+1u]=a.status;
    output[i+2u]=bitcast<u32>(g.value.x);output[i+3u]=bitcast<u32>(g.value.y);output[i+4u]=bitcast<u32>(g.value.z);output[i+5u]=g.status;
    output[i+6u]=bitcast<u32>(b.value);output[i+7u]=b.status;
    output[i+8u]=bitcast<u32>(h.value.x);output[i+9u]=bitcast<u32>(h.value.y);output[i+10u]=bitcast<u32>(h.value.z);output[i+11u]=h.status;
}
