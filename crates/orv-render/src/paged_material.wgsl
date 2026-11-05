// SPDX-License-Identifier: GPL-3.0-or-later
struct RayProgress {radiance:f32,transmittance:f32,index:u32,phase:u32}
@group(1) @binding(0) var<uniform> p:Params;
@group(1) @binding(1) var output:texture_storage_2d<rgba16float,write>;
@group(1) @binding(2) var<storage,read_write> rays:array<RayProgress>;
// Error, pending rays, advanced source indices, then required-page bitset.
@group(1) @binding(3) var<storage,read_write> progress:array<atomic<u32>>;
fn finite_scalar(v:f32)->bool {return v==v && abs(v)<=3.402823e38;}
fn fail_ray(){atomicOr(&progress[0],1u);}
fn paged_illumination(point:vec3f,page:u32)->f32 {
    if p.optics.z==0. {return p.optics.y;}
    let g=atlas_gradient_from_known(point,page);
    if g.status==BRICK_MISSING {return 1.;}
    if g.status!=BRICK_KNOWN {fail_ray();return 0.;}
    var index_gradient=g.value;
    let scale=max(max(abs(index_gradient.x),abs(index_gradient.y)),abs(index_gradient.z));
    if scale==0. {return 1.;}
    index_gradient/=scale;
    var normal=vec3f(dot(index_gradient,p.inverse_x.xyz),dot(index_gradient,p.inverse_y.xyz),dot(index_gradient,p.inverse_z.xyz));
    let normal_scale=max(max(abs(normal.x),abs(normal.y)),abs(normal.z));
    if normal_scale==0. {return 1.;}
    normal=normalize(normal/normal_scale);
    return p.optics.y+p.optics.z*abs(dot(normal,p.light.xyz));
}

fn advance_ray(origin:vec3f,ray:vec2f,input:RayProgress,track:bool)->RayProgress {
    var state=input;
    if !finite_scalar(ray.x) || !finite_scalar(ray.y) {fail_ray();return state;}
    let initial=state.index;
    if ray.y>=ray.x {
        let length=max(ray.y-ray.x,0.);
        if !finite_scalar(length) {fail_ray();return state;}
        let count=u32(clamp(ceil(length/p.sampling.x),1.,p.sampling.y));
        let step=length/f32(count);
        if !finite_scalar(step) || state.index>count {fail_ray();return state;}
        var i=state.index;
        while i<count {
            // Recreate the original whole-ray expression; pages never reset it.
            let point=material_point(origin,ray.x,step,i);
            if !all(point==point) || any(abs(point)>vec3f(3.402823e38)) {fail_ray();return state;}
            let query=clamp(point,vec3f(0.),vec3f(source_dimensions())-1.);
            // A certified zero has the same optical contribution whether its
            // scalar stencil is valid or missing. It needs no resident page.
            let advance=transparent_advance(vec3u(floor(query)),point,origin,ray.x,step,i,count);
            if advance!=0u {i+=advance;state.index=i;continue;}
            let value=atlas_scalar(query);
            if value.status==BRICK_NONRESIDENT {
                if track && value.page<atlas.stored.w {
                    atomicOr(&progress[3u+value.page/32u],1u<<(value.page%32u));
                    atomicAdd(&progress[1],1u);
                    atomicAdd(&progress[2],state.index-initial);
                } else {fail_ray();}
                return state;
            }
            i++;state.index=i;
            if value.status==BRICK_MISSING {continue;}
            if value.status!=BRICK_KNOWN {fail_ray();return state;}
            let optics=material(value.value);
            let depth=optics.y*step;
            let alpha=select(1.-exp(-depth),depth*(1.-depth*(0.5-depth/6.)),depth<0.001);
            if alpha<=0. {continue;}
            state.radiance+=state.transmittance*alpha*optics.x*paged_illumination(query,value.page);
            state.transmittance*=1.-alpha;
            if state.transmittance<0.0001 {break;}
        }
    }
    if track {atomicAdd(&progress[2],state.index-initial);}
    state.phase=2u;return state;
}
fn finish_pixel(pixel:vec2u,state:RayProgress) {
    if state.phase!=2u {return;}
    let result=state.radiance+state.transmittance*0.0030352698;
    if !finite_scalar(result) || result<0. || result>1.0001 || state.transmittance<0. || state.transmittance>1.0001 {fail_ray();return;}
    textureStore(output,vec2i(pixel),vec4f(result,result,result,1.));
}
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2u(p.region.xy)+gid.xy;
    let address=gid.y*u32(p.region.z)+gid.x;
    var state=rays[address];
    if state.phase==2u {return;}
    if state.phase==0u {state.transmittance=1.;state.phase=1u;}
    let uv=(vec2f(pixel)+0.5)/vec2f(textureDimensions(output));
    let origin=p.origin.xyz+p.right.xyz*(uv.x-0.5)+p.down.xyz*(uv.y-0.5);
    state=advance_ray(origin,clipped_interval(uv,interval(origin,p.direction.xyz)),state,true);
    rays[address]=state;finish_pixel(pixel,state);
}
// Every required source page is resident. Certified zeros need no page. No ray-state or
// global progress counter is accessed on the normal complete-image path.
@compute @workgroup_size(8,8,1)
fn resident(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2u(p.region.xy)+gid.xy;
    let uv=(vec2f(pixel)+0.5)/vec2f(textureDimensions(output));
    let origin=p.origin.xyz+p.right.xyz*(uv.x-0.5)+p.down.xyz*(uv.y-0.5);
    let state=advance_ray(origin,clipped_interval(uv,interval(origin,p.direction.xyz)),RayProgress(0.,1.,0u,1u),false);
    finish_pixel(pixel,state);
}
