// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    origin:vec4f, right:vec4f, down:vec4f, direction:vec4f,
    display:vec4f, transfer:vec4f, sampling:vec4f, region:vec4f, voi:vec4f,
    world_origin:vec4f, world_right:vec4f, world_down:vec4f, world_direction:vec4f,
    clip_minimum:vec4f, clip_maximum:vec4f,
    inverse_x:vec4f, inverse_y:vec4f, inverse_z:vec4f, light:vec4f,
    optics:vec4f, points:array<vec4f,64>,
}
@group(0) @binding(0) var source:texture_3d<f32>;
@group(0) @binding(1) var<uniform> p:Params;
@group(0) @binding(2) var output:texture_storage_2d<rgba16float,write>;
@group(0) @binding(3) var validity:texture_3d<u32>;

fn material(value:f32)->vec2f {
    var a=p.points[0];
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
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2u(p.region.xy)+gid.xy;
    let uv=(vec2f(pixel)+0.5)/vec2f(textureDimensions(output));
    let origin=p.origin.xyz+p.right.xyz*(uv.x-0.5)+p.down.xyz*(uv.y-0.5);
    let ray=clipped_interval(uv,interval(origin,p.direction.xyz));
    var transmittance=1.;var radiance=0.;
    if ray.y>=ray.x {
        let length=max(ray.y-ray.x,0.);
        let count=u32(clamp(ceil(length/p.sampling.x),1.,p.sampling.y));
        let step=length/f32(count);
        for(var i=0u;i<count;i+=1u) {
            let point=origin+p.direction.xyz*(ray.x+(f32(i)+0.5)*step);
            if !scalar_valid(point) {continue;}
            let optics=material(scalar(point));
            // Stable for small optical depth, where 1-exp(-x) loses precision.
            let depth=optics.y*step;
            let alpha=select(1.-exp(-depth),depth*(1.-depth*(0.5-depth/6.)),depth<0.001);
            if alpha<=0. {continue;}
            radiance+=transmittance*alpha*optics.x*illumination(point);
            transmittance*=1.-alpha;
            if transmittance<0.0001 {break;}
        }
    }
    let result=radiance+transmittance*0.0030352698;
    textureStore(output,vec2i(pixel),vec4f(result,result,result,1.));
}
