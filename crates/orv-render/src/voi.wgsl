// SPDX-License-Identifier: GPL-3.0-or-later
fn analytic_voi(value:f32,width:f32,center:f32,kind:f32)->f32 {
    // Division may compile to a reciprocal, and 1/width is subnormal for very
    // wide float windows; a flushed reciprocal would collapse every sample to
    // the window centre. Exact power-of-two rescaling keeps the ratio intact.
    let shift=select(0,frexp(width).exp,width>1e18);
    let v=ldexp(value,-shift);
    let c=ldexp(center,-shift);
    let w=ldexp(width,-shift);
    if kind>1.5 {
        let t=clamp((v-c)/w,-20.,20.);
        return 1./(1.+exp(-4.*t));
    }
    if kind>0.5 { return clamp((v-c)/w+0.5,0.,1.); }
    if width<=1. { return select(0.,1.,value>center-0.5); }
    let half=ldexp(0.5,-shift);
    return clamp((v-(c-half))/(w-2.*half)+0.5,0.,1.);
}
