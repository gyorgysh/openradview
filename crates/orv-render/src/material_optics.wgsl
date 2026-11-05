// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(0) var source:texture_3d<f32>;
@group(0) @binding(1) var<uniform> p:Params;
@group(0) @binding(3) var validity:texture_3d<u32>;

fn illumination(point:vec3f)->f32 {
    if p.optics.z==0. {return p.optics.y;}
    let maximum=vec3f(textureDimensions(source))-1.;
    let q=clamp(point,vec3f(0.),maximum);
    var index_gradient=vec3f(0.);
    for(var axis=0u;axis<3u;axis+=1u) {
        var lo=q;var hi=q;
        lo[axis]=max(q[axis]-1.,0.);hi[axis]=min(q[axis]+1.,maximum[axis]);
        // Missing stencils and constant fields keep unlit material. No invented
        // normal at a padding boundary, and no intensity-unit gradient cutoff.
        if !scalar_valid(lo) || !scalar_valid(hi) {return 1.;}
        index_gradient[axis]=(scalar(hi)-scalar(lo))/(hi[axis]-lo[axis]);
    }
    let scale=max(max(abs(index_gradient.x),abs(index_gradient.y)),abs(index_gradient.z));
    if scale==0. {return 1.;}
    index_gradient/=scale;
    var normal=vec3f(dot(index_gradient,p.inverse_x.xyz),dot(index_gradient,p.inverse_y.xyz),dot(index_gradient,p.inverse_z.xyz));
    let normal_scale=max(max(abs(normal.x),abs(normal.y)),abs(normal.z));
    if normal_scale==0. {return 1.;}
    normal=normalize(normal/normal_scale);
    return p.optics.y+p.optics.z*abs(dot(normal,p.light.xyz));
}
// Integrate only the declared scalar-centre interval. Invalid gaps are transparent.
fn integrate_material(origin:vec3f,ray:vec2f)->vec2f {
    var transmittance=1.;var radiance=0.;
    if ray.y>=ray.x {
        let length=max(ray.y-ray.x,0.);
        let count=u32(clamp(ceil(length/p.sampling.x),1.,p.sampling.y));
        let step=length/f32(count);
        for(var i=0u;i<count;) {
            let point=material_point(origin,ray.x,step,i);
            let sample=optical_sample(point,origin,ray.x,step,i,count);
            i+=sample.advance;
            let optics=sample.optics;
            // Stable for small optical depth, where 1-exp(-x) loses precision.
            let depth=optics.y*step;
            let alpha=select(1.-exp(-depth),depth*(1.-depth*(0.5-depth/6.)),depth<0.001);
            if alpha<=0. {continue;}
            radiance+=transmittance*alpha*optics.x*illumination(point);
            transmittance*=1.-alpha;
            if transmittance<0.0001 {break;}
        }
    }
    return vec2f(radiance,transmittance);
}
