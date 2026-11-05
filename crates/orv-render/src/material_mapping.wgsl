// SPDX-License-Identifier: GPL-3.0-or-later
fn material_point(origin:vec3f,near:f32,step:f32,index:u32)->vec3f {
    return origin+p.direction.xyz*(near+(f32(index)+.5)*step);
}
fn material(value:f32)->vec2f {
    var a=p.points[0];
    // Clamp endpoint values before division, avoiding
    // overflow for a distant source value and closely spaced material knots.
    if value<=a.x {return a.yz;}
    for(var i=1u;i<u32(p.optics.x);i+=1u) {
        let b=p.points[i];
        if value<=b.x {
            let t=clamp((value-a.x)/(b.x-a.x),0.,1.);
            return mix(a.yz,b.yz,t);
        }
        a=b;
    }
    return a.yz;
}
