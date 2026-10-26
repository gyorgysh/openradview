// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var interpolation:sampler;
@group(0) @binding(2) var<uniform> color_space:vec4f;
struct Vertex { @builtin(position) position:vec4f, @location(0) uv:vec2f }
@vertex fn vs(@builtin(vertex_index) index:u32)->Vertex {
    let xy=array<vec2f,3>(vec2f(-1.,-1.),vec2f(3.,-1.),vec2f(-1.,3.));
    var out:Vertex; out.position=vec4f(xy[index],0.,1.);
    out.uv=vec2f((xy[index].x+1.)*0.5,(1.-xy[index].y)*0.5); return out;
}
@fragment fn fs(in:Vertex)->@location(0) vec4f {
    // The display view is sRGB: hardware decodes before linear interpolation.
    let linear=textureSample(image,interpolation,in.uv).rgb;
    let encoded=select(linear*12.92,1.055*pow(max(linear,vec3f(0.)),vec3f(1./2.4))-0.055,linear>vec3f(0.0031308));
    return vec4f(select(encoded,linear,color_space.x>0.5),1.);
}
