// SPDX-License-Identifier: GPL-3.0-or-later
struct OpticalSample {optics:vec2f,advance:u32}
fn material_point(origin:vec3f,near:f32,step:f32,index:u32)->vec3f {
    return origin+p.direction.xyz*(near+(f32(index)+.5)*step);
}
fn optical_sample(point:vec3f,origin:vec3f,near:f32,step:f32,index:u32,count:u32)->OpticalSample {
    if SHARED_OPTICAL_STENCIL {
        let stencil=scalar_stencil(point);
        // The skip and the actual loads use this same integer stencil. No
        // independent rounded ray/owner calculation can cross a tile boundary.
        if !stencil_valid(stencil) {return OpticalSample(vec2f(0.),1u);}
        let advance=transparent_advance(stencil.lower,point,origin,near,step,index,count);
        if advance!=0u {return OpticalSample(vec2f(0.),advance);}
        return OpticalSample(material(stencil_scalar(stencil)),1u);
    }
    if !scalar_valid(point) {return OpticalSample(vec2f(0.),1u);}
    return OpticalSample(material(scalar(point)),1u);
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
