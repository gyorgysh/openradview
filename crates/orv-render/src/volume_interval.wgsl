// SPDX-License-Identifier: GPL-3.0-or-later
fn interval(origin:vec3f,direction:vec3f)->vec2f {
    let maximum=vec3f(source_dimensions())-1.;
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
