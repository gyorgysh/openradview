// SPDX-License-Identifier: GPL-3.0-or-later
struct OpticalSample {optics:vec2f,advance:u32}
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
