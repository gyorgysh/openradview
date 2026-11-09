// SPDX-License-Identifier: GPL-3.0-or-later
fn source_interval(origin:vec3f,direction:vec3f,offset:vec2f)->vec2f {
    let maximum=vec3f(textureDimensions(source))-1.;
    var near=-1.e30;var far=1.e30;
    for(var axis=0u;axis<3u;axis+=1u) {
        if direction[axis]==0. {
            if origin[axis]<0. || origin[axis]>maximum[axis] {return vec2f(1.,-1.);}
        } else {
            let a=-origin[axis]/direction[axis];let b=(maximum[axis]-origin[axis])/direction[axis];
            near=max(near,min(a,b));far=min(far,max(a,b));
        }
    }
    if p.clip_minimum.w>0.5 {
        let world=p.world_origin.xyz+p.world_right.xyz*offset.x+p.world_down.xyz*offset.y;
        for(var axis=0u;axis<3u;axis+=1u) {
            let delta=p.world_direction[axis];
            if delta==0. {
                if world[axis]<p.clip_minimum[axis] || world[axis]>p.clip_maximum[axis] {return vec2f(1.,-1.);}
            } else {
                let a=(p.clip_minimum[axis]-world[axis])/delta;let b=(p.clip_maximum[axis]-world[axis])/delta;
                near=max(near,min(a,b));far=min(far,max(a,b));
            }
        }
    }
    return vec2f(near,far);
}
