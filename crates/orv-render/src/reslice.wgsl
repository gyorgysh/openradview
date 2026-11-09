// SPDX-License-Identifier: GPL-3.0-or-later
struct Params { origin:vec4f, du:vec4f, dv:vec4f, normal:vec4f, slab:vec4f, region:vec4f }
@group(0) @binding(0) var source:texture_3d<f32>;
@group(0) @binding(1) var validity:texture_3d<u32>;
@group(0) @binding(2) var<uniform> p:Params;
@group(0) @binding(3) var output:texture_storage_2d<rgba32float,write>;

// Interpolate scalar values before display mapping. A missing, nonzero-weight
// contributor invalidates the sample; a zero-weight missing neighbor does not.
fn sample_scalar(point:vec3f)->vec2f {
    let maximum=vec3i(textureDimensions(source))-1;
    if any(point<vec3f(-1.e-6)) || any(point>vec3f(maximum)+vec3f(1.e-6)) { return vec2f(0.); }
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3i(floor(q));let b=min(a+1,maximum);let t=fract(q);
    var value=0.;
    for(var z=0u;z<2u;z+=1u) {
        for(var y=0u;y<2u;y+=1u) {
            for(var x=0u;x<2u;x+=1u) {
                let hi=vec3<bool>(x==1u,y==1u,z==1u);
                let w=select(vec3f(1.)-t,t,hi);
                let weight=w.x*w.y*w.z;
                if weight==0. { continue; }
                let index=select(a,b,hi);
                if p.origin.w>0.5 && (textureLoad(validity,index,0).r & 1u)==0u { return vec2f(0.); }
                value+=weight*textureLoad(source,index,0).r;
            }
        }
    }
    return vec2f(value,1.);
}
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) { return; }
    let pixel=vec2u(p.region.xy)+gid.xy;
    let center=p.origin.xyz+p.du.xyz*f32(pixel.x)+p.dv.xyz*f32(pixel.y);
    var result=vec2f(0.);
    if p.slab.z<0.5 { result=sample_scalar(center); }
    else {
        var count=0u;var value=0.;
        for(var i=0u;i<=u32(p.slab.y);i+=1u) {
            let distance=p.slab.x*(f32(i)/p.slab.y-0.5);
            let sample=sample_scalar(center+p.normal.xyz*distance);
            if sample.y<0.5 { continue; }
            if p.slab.z>2.5 {
                // Normalize before accumulation to keep finite source extrema
                // inside the admitted scalar range even for 257 samples.
                value+=sample.x/(p.slab.y+1.);
            } else if count==0u { value=sample.x; }
            else if p.slab.z<1.5 { value=max(value,sample.x); }
            else { value=min(value,sample.x); }
            count+=1u;
        }
        if count>0u {
            if p.slab.z>2.5 { value=value*((p.slab.y+1.)/f32(count)); }
            result=vec2f(value,1.);
        }
    }
    textureStore(output,vec2i(pixel),vec4f(result,0.,1.));
}
