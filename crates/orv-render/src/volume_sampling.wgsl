// SPDX-License-Identifier: GPL-3.0-or-later
// Shared affine interval and validity-aware scalar sampling.
fn interval(origin:vec3f,direction:vec3f)->vec2f {
    let maximum=vec3f(textureDimensions(source))-1.;
    var near=-1.e30; var far=1.e30;
    for(var axis=0u;axis<3u;axis+=1u) {
        if abs(direction[axis])<1.e-12 {
            if origin[axis]<0. || origin[axis]>maximum[axis] { return vec2f(1.,-1.); }
        } else {
            let a=-origin[axis]/direction[axis];
            let b=(maximum[axis]-origin[axis])/direction[axis];
            near=max(near,min(a,b)); far=min(far,max(a,b));
        }
    }
    return vec2f(near,far);
}
fn clipped_interval(uv:vec2f,ray:vec2f)->vec2f {
    if p.clip_minimum.w<0.5 {return ray;}
    let origin=p.world_origin.xyz+p.world_right.xyz*(uv.x-0.5)+p.world_down.xyz*(uv.y-0.5);
    var near=ray.x;var far=ray.y;
    for(var axis=0u;axis<3u;axis+=1u) {
        let direction=p.world_direction[axis];
        if abs(direction)<1.e-12 {
            if origin[axis]<p.clip_minimum[axis] || origin[axis]>p.clip_maximum[axis] {return vec2f(1.,-1.);}
        } else {
            let a=(p.clip_minimum[axis]-origin[axis])/direction;
            let b=(p.clip_maximum[axis]-origin[axis])/direction;
            near=max(near,min(a,b));far=min(far,max(a,b));
        }
    }
    return vec2f(near,far);
}
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
fn scalar_valid(point:vec3f)->bool {
    if p.sampling.w<0.5 {return true;}
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3i(floor(q));let b=min(a+1,maximum);let t=fract(q);
    for(var z=0u;z<2u;z+=1u) {
        for(var y=0u;y<2u;y+=1u) {
            for(var x=0u;x<2u;x+=1u) {
                let hi=vec3<bool>(x==1u,y==1u,z==1u);
                let w=select(vec3f(1.)-t,t,hi);
                if w.x*w.y*w.z>0. && textureLoad(validity,select(a,b,hi),0).r==0u {return false;}
            }
        }
    }
    return true;
}
