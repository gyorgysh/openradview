// SPDX-License-Identifier: GPL-3.0-or-later
struct Params { window: vec4f, geometry: vec4f, output: vec4f }
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var<uniform> p: Params;
@group(0) @binding(2) var valid: texture_2d<u32>;
struct Vertex { @builtin(position) position: vec4f, @location(0) uv: vec2f }
@vertex fn vs(@builtin(vertex_index) index: u32) -> Vertex {
    let xy = array<vec2f, 3>(vec2f(-1., -1.), vec2f(3., -1.), vec2f(-1., 3.));
    var out: Vertex;
    out.position = vec4f(xy[index], 0., 1.);
    out.uv = vec2f((xy[index].x + 1.) * 0.5, (1. - xy[index].y) * 0.5);
    return out;
}
fn output_rgb(rgb: vec3f) -> vec4f {
    let linear = select(rgb/12.92, pow((rgb+0.055)/1.055, vec3f(2.4)), rgb>vec3f(0.04045));
    return vec4f(select(rgb, linear, p.output.x>0.5),1.);
}
fn output_gray(g: f32) -> vec4f { return output_rgb(vec3f(g)); }
fn encode_rgb(linear:vec3f)->vec3f {
    return select(linear*12.92,1.055*pow(max(linear,vec3f(0.)),vec3f(1./2.4))-0.055,linear>vec3f(0.0031308));
}
fn sample_scalar(uv: vec2f) -> vec4f {
    let size = vec2i(textureDimensions(image));
    let raw_xy = uv*vec2f(size)-0.5;
    // Interpolated UVs can miss an exact pixel centre by a few float ULPs.
    // Snap within 0.00001 pixel so a zero-weight no-data neighbour stays zero.
    let nearest = round(raw_xy);
    let xy = select(raw_xy,nearest,abs(raw_xy-nearest)<vec2f(0.00001));
    let base = vec2i(floor(xy));
    let blend = fract(xy);
    let a=weighted(clamp(base,vec2i(0),size-1),(1.-blend.x)*(1.-blend.y));
    let b=weighted(clamp(base+vec2i(1,0),vec2i(0),size-1),blend.x*(1.-blend.y));
    let c=weighted(clamp(base+vec2i(0,1),vec2i(0),size-1),(1.-blend.x)*blend.y);
    let d=weighted(clamp(base+vec2i(1,1),vec2i(0),size-1),blend.x*blend.y);
    return vec4f(a.rgb+b.rgb+c.rgb+d.rgb,min(min(a.a,b.a),min(c.a,d.a)));
}
fn weighted(xy:vec2i,weight:f32)->vec4f {
    if weight<=0. { return vec4f(0.,0.,0.,1.); }
    // An integer mask avoids relying on NaN behavior under WGSL finite-math optimizations.
    if textureLoad(valid,xy,0).r==0u { return vec4f(0.); }
    return vec4f(textureLoad(image,xy,0).rgb*weight,1.);
}
@fragment fn fs(in: Vertex) -> @location(0) vec4f {
    let image_aspect = p.geometry.z;
    let viewport_aspect = p.geometry.w;
    let fit=select(vec2f(image_aspect/viewport_aspect,1.),
                   vec2f(1.,viewport_aspect/image_aspect),image_aspect>viewport_aspect);
    let uv=(in.uv-0.5-p.geometry.xy)/(fit*p.window.w)+0.5;
    if any(uv<vec2f(0.)) || any(uv>=vec2f(1.)) { return output_gray(10./255.); }
    let sample=sample_scalar(uv);
    if sample.a<0.5 { return output_gray(10./255.); }
    if p.output.y>0.5 {
        let encoded=encode_rgb(sample.rgb);
        if p.window.z>0.5 { return output_rgb(vec3f(1.)-encoded); }
        return vec4f(select(encoded,sample.rgb,p.output.x>0.5),1.);
    }
    let value=sample.x;
    var gray=select(clamp((value-(p.window.y-0.5))/max(p.window.x-1.,0.0001)+0.5,0.,1.),
                    select(0.,1.,value>p.window.y-0.5),p.window.x<=1.);
    if p.window.z>0.5 { gray=1.-gray; }
    return output_gray(gray);
}
