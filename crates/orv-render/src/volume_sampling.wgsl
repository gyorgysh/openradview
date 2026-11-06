// SPDX-License-Identifier: GPL-3.0-or-later
// Shared affine interval and validity-aware scalar sampling.
fn source_dimensions()->vec3u {return textureDimensions(source);}
fn scalar(point:vec3f)->f32 {
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3i(floor(q)); let b=min(a+1,maximum); let t=fract(q);
    let c00=mix(textureLoad(source,a,0).r,textureLoad(source,vec3i(b.x,a.y,a.z),0).r,t.x);
    let c10=mix(textureLoad(source,vec3i(a.x,b.y,a.z),0).r,textureLoad(source,vec3i(b.x,b.y,a.z),0).r,t.x);
    let c01=mix(textureLoad(source,vec3i(a.x,a.y,b.z),0).r,textureLoad(source,vec3i(b.x,a.y,b.z),0).r,t.x);
    let c11=mix(textureLoad(source,vec3i(a.x,b.y,b.z),0).r,textureLoad(source,b,0).r,t.x);
    return mix(mix(c00,c10,t.y),mix(c01,c11,t.y),t.z);
}
// Each byte retains the source validity at bit zero and its positive stencil
// in bits x+2*y+4*z. Duplicate edge nodes are packed with clamped coordinates.
fn packed_stencil_valid(lower:vec3i,t:vec3f)->bool {
    let bits=textureLoad(validity,lower,0).r;
    if bits==255u {return true;}
    if bits==0u {return false;}
    for(var z=0u;z<2u;z+=1u) {
        for(var y=0u;y<2u;y+=1u) {
            for(var x=0u;x<2u;x+=1u) {
                let hi=vec3<bool>(x==1u,y==1u,z==1u);
                let w=select(vec3f(1.)-t,t,hi);
                if w.x*w.y*w.z>0. && (bits & (1u<<(x+2u*y+4u*z)))==0u {return false;}
            }
        }
    }
    return true;
}
fn scalar_valid(point:vec3f)->bool {
    if p.sampling.w<0.5 {return true;}
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    return packed_stencil_valid(vec3i(floor(q)),fract(q));
}
