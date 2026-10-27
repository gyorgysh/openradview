// SPDX-License-Identifier: GPL-3.0-or-later
fn analytic_voi(value:f32,width:f32,center:f32,kind:f32)->f32 {
    if kind>1.5 {
        let t=clamp((value-center)/width,-20.,20.);
        return 1./(1.+exp(-4.*t));
    }
    if kind>0.5 { return clamp((value-center)/width+0.5,0.,1.); }
    if width<=1. { return select(0.,1.,value>center-0.5); }
    return clamp((value-(center-0.5))/(width-1.)+0.5,0.,1.);
}
