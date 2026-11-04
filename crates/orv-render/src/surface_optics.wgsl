// SPDX-License-Identifier: GPL-3.0-or-later
// Midpoint validity cannot weight an optical step that crosses an invalid
// interpolation stencil. Split at the actual source-grid node planes first.
fn integrate_surface_source(origin:vec3f,ray:vec2f)->vec2f {
    if p.sampling.w<.5 {return integrate_material(origin,ray);}
    if ray.y<=ray.x {return vec2f(0.,1.);}
    var nodes=vec3f(0.);var times=vec3f(1e30);
    for(var a=0u;a<3u;a++) {
        let direction=p.direction[a];
        if direction==0. {continue;}
        let point=origin[a]+direction*ray.x;
        nodes[a]=select(ceil(point)-1.,floor(point)+1.,direction>0.);
        var resolved=false;
        // Resolve the entry with boundary parameters, without a physical nudge
        // that could erase a thin valid interval or cross a clipping boundary.
        for(var attempt=0u;attempt<3u;attempt++) {
            times[a]=(nodes[a]-origin[a])/direction;
            let previous=(nodes[a]-sign(direction)-origin[a])/direction;
            if previous>ray.x {nodes[a]-=sign(direction);}
            else if times[a]<=ray.x {nodes[a]+=sign(direction);}
            else {resolved=true;break;}
        }
        if !resolved {atomicStore(&failed,1u);return vec2f(0.,1.);}
    }
    var cursor=ray.x;var radiance=0.;var transmission=1.;
    let dimensions=textureDimensions(source);
    let limit=dimensions.x+dimensions.y+dimensions.z+3u;
    for(var visit=0u;visit<limit;visit++) {
        let end=min(ray.y,min(min(times.x,times.y),times.z));
        if !(end>cursor) {atomicStore(&failed,1u);return vec2f(0.,1.);}
        let midpoint=origin+p.direction.xyz*(cursor+(end-cursor)*.5);
        if scalar_valid(midpoint) {
            let part=integrate_material(origin,vec2f(cursor,end));
            radiance+=transmission*part.x;transmission*=part.y;
        }
        if end==ray.y || transmission<.0001 {return vec2f(radiance,transmission);}
        var advanced=false;
        for(var a=0u;a<3u;a++) {
            if p.direction[a]!=0. && times[a]<=end {
                nodes[a]+=sign(p.direction[a]);times[a]=(nodes[a]-origin[a])/p.direction[a];advanced=true;
            }
        }
        if !advanced {atomicStore(&failed,1u);return vec2f(0.,1.);}
        cursor=end;
    }
    atomicStore(&failed,1u);return vec2f(0.,1.);
}
