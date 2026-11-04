// SPDX-License-Identifier: GPL-3.0-or-later
// The classifier and unchanged trilinear loads share one integer stencil.
struct ScalarStencil {lower:vec3u,upper:vec3u,fraction:vec3f}
fn scalar_stencil(point:vec3f)->ScalarStencil {
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3u(floor(q));let b=min(a+1u,vec3u(maximum));
    return ScalarStencil(a,b,fract(q));
}
fn stencil_scalar(stencil:ScalarStencil)->f32 {
    let a=vec3i(stencil.lower);let b=vec3i(stencil.upper);let t=stencil.fraction;
    let v0=textureLoad(source,a,0).r;
    let v1=textureLoad(source,vec3i(b.x,a.y,a.z),0).r;
    let v2=textureLoad(source,vec3i(a.x,b.y,a.z),0).r;
    let v3=textureLoad(source,vec3i(b.x,b.y,a.z),0).r;
    let v4=textureLoad(source,vec3i(a.x,a.y,b.z),0).r;
    let v5=textureLoad(source,vec3i(b.x,a.y,b.z),0).r;
    let v6=textureLoad(source,vec3i(a.x,b.y,b.z),0).r;
    let v7=textureLoad(source,b,0).r;
    let value=mix(mix(mix(v0,v1,t.x),mix(v2,v3,t.x),t.y),
                  mix(mix(v4,v5,t.x),mix(v6,v7,t.x),t.y),t.z);
    return value;
}
fn stencil_valid(stencil:ScalarStencil)->bool {
    if p.sampling.w<.5 {return true;}
    for(var z=0u;z<2u;z++) {for(var y=0u;y<2u;y++) {for(var x=0u;x<2u;x++) {
        let hi=vec3<bool>(x==1u,y==1u,z==1u);
        let w=select(vec3f(1.)-stencil.fraction,stencil.fraction,hi);
        if w.x*w.y*w.z>0. && textureLoad(validity,vec3i(select(stencil.lower,stencil.upper,hi)),0).r==0u {return false;}
    }}}
    return true;
}
