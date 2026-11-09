// SPDX-License-Identifier: GPL-3.0-or-later
// Global interpolation stencils precede integer atlas addressing. Caller points
// must be finite. Nonresident status carries the required source-page index.
struct AtlasParameters {dimensions:vec4u,counts:vec4u,grid:vec4u,stored:vec4u}
struct AtlasPage {origin_slot:vec4u,size_stencil:vec4u}
struct AtlasValue {value:f32,status:u32,page:u32}
struct AtlasGradient {value:vec3f,status:u32,page:u32}
@group(0) @binding(0) var atlas_values:texture_3d<f32>;
@group(0) @binding(1) var atlas_validity:texture_3d<u32>;
@group(0) @binding(2) var<uniform> atlas:AtlasParameters;
@group(0) @binding(3) var<storage,read> atlas_pages:array<AtlasPage>;
fn source_dimensions()->vec3u {return atlas.dimensions.xyz;}
fn atlas_page_index(point:vec3f)->u32 {
    let q=clamp(point,vec3f(0.),vec3f(atlas.dimensions.xyz)-1.);
    let cell=min(vec3u(floor(q)),atlas.dimensions.xyz-2u)/atlas.counts.w;
    return (cell.z*atlas.counts.y+cell.y)*atlas.counts.x+cell.x;
}
fn atlas_slot_origin(slot:u32)->vec3u {
    return vec3u(slot%atlas.grid.x,slot/atlas.grid.x%atlas.grid.y,slot/(atlas.grid.x*atlas.grid.y))*atlas.stored.xyz;
}
fn atlas_stencil(point:vec3f,page:u32)->AtlasValue {
    let entry=atlas_pages[page];
    if entry.origin_slot.w==0u {return AtlasValue(0.,BRICK_NONRESIDENT,page);}
    let maximum=atlas.dimensions.xyz-1u;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let ga=vec3u(floor(q));let gb=min(ga+1u,maximum);let t=fract(q);
    if any(ga<entry.origin_slot.xyz) || any(gb>=entry.origin_slot.xyz+entry.size_stencil.xyz) {
        return AtlasValue(0.,BRICK_STENCIL_UNAVAILABLE,page);
    }
    let origin=atlas_slot_origin(entry.origin_slot.w-1u);
    let a=vec3i(origin+ga-entry.origin_slot.xyz);let b=vec3i(origin+gb-entry.origin_slot.xyz);
    if atlas.dimensions.w!=0u {
        for(var z=0u;z<2u;z++){for(var y=0u;y<2u;y++){for(var x=0u;x<2u;x++){
            let hi=vec3<bool>(x==1u,y==1u,z==1u);let w=select(vec3f(1.)-t,t,hi);
            if w.x*w.y*w.z>0. && textureLoad(atlas_validity,select(a,b,hi),0).r==0u {return AtlasValue(0.,BRICK_MISSING,page);}
        }}}
    }
    let c00=scalar_mix(textureLoad(atlas_values,a,0).r,textureLoad(atlas_values,vec3i(b.x,a.y,a.z),0).r,t.x);
    let c10=scalar_mix(textureLoad(atlas_values,vec3i(a.x,b.y,a.z),0).r,textureLoad(atlas_values,vec3i(b.x,b.y,a.z),0).r,t.x);
    let c01=scalar_mix(textureLoad(atlas_values,vec3i(a.x,a.y,b.z),0).r,textureLoad(atlas_values,vec3i(b.x,a.y,b.z),0).r,t.x);
    let c11=scalar_mix(textureLoad(atlas_values,vec3i(a.x,b.y,b.z),0).r,textureLoad(atlas_values,b,0).r,t.x);
    return AtlasValue(scalar_mix(scalar_mix(c00,c10,t.y),scalar_mix(c01,c11,t.y),t.z),BRICK_KNOWN,page);
}
fn atlas_scalar(point:vec3f)->AtlasValue {
    let maximum=vec3f(atlas.dimensions.xyz)-1.;
    if any(point<vec3f(-0.000001)) || any(point>maximum+vec3f(0.000001)) {return AtlasValue(0.,BRICK_OUTSIDE_SOURCE,0u);}
    return atlas_stencil(point,atlas_page_index(point));
}
fn atlas_index_gradient(point:vec3f)->AtlasGradient {
    let centre=atlas_scalar(point);
    if centre.status!=BRICK_KNOWN {return AtlasGradient(vec3f(0.),centre.status,centre.page);}
    return atlas_gradient_from_known(point,centre.page);
}
// The caller already proved this centre known using the same snapshot/stencil.
fn atlas_gradient_from_known(point:vec3f,page:u32)->AtlasGradient {
    if atlas_pages[page].size_stencil.w!=1u {return AtlasGradient(vec3f(0.),BRICK_STENCIL_UNAVAILABLE,page);}
    let maximum=vec3f(atlas.dimensions.xyz)-1.;let q=clamp(point,vec3f(0.),maximum);var gradient=vec3f(0.);
    for(var axis=0u;axis<3u;axis++){
        var lo=q;var hi=q;lo[axis]=max(q[axis]-1.,0.);hi[axis]=min(q[axis]+1.,maximum[axis]);
        let a=atlas_stencil(lo,page);let b=atlas_stencil(hi,page);
        if a.status!=BRICK_KNOWN {return AtlasGradient(vec3f(0.),a.status,page);}
        if b.status!=BRICK_KNOWN {return AtlasGradient(vec3f(0.),b.status,page);}
        gradient[axis]=(b.value-a.value)/(hi[axis]-lo[axis]);
    }
    return AtlasGradient(gradient,BRICK_KNOWN,page);
}
