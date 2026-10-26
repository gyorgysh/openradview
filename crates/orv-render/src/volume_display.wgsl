// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var interpolation:sampler;
@group(0) @binding(2) var<uniform> color_space:vec4f;
override manual_srgb:bool=false;
struct Vertex { @builtin(position) position:vec4f, @location(0) uv:vec2f }
@vertex fn vs(@builtin(vertex_index) index:u32)->Vertex {
    let xy=array<vec2f,3>(vec2f(-1.,-1.),vec2f(3.,-1.),vec2f(-1.,3.));
    var out:Vertex; out.position=vec4f(xy[index],0.,1.);
    out.uv=vec2f((xy[index].x+1.)*0.5,(1.-xy[index].y)*0.5); return out;
}
fn texel_linear(pixel:vec2i,size:vec2i)->vec3f {
    let encoded=textureLoad(image,clamp(pixel,vec2i(0),size-1),0).rgb;
    return select(encoded/12.92,pow((encoded+0.055)/1.055,vec3f(2.4)),encoded>vec3f(0.04045));
}
fn sample_linear(uv:vec2f)->vec3f {
    if !manual_srgb {
        // Hardware decodes the sRGB view before interpolation.
        return textureSample(image,interpolation,uv).rgb;
    }
    let size=vec2i(textureDimensions(image));
    let xy=uv*vec2f(size)-0.5;
    let base=vec2i(floor(xy)); let t=fract(xy);
    return mix(mix(texel_linear(base,size),texel_linear(base+vec2i(1,0),size),t.x),
               mix(texel_linear(base+vec2i(0,1),size),texel_linear(base+vec2i(1,1),size),t.x),t.y);
}
@fragment fn fs(in:Vertex)->@location(0) vec4f {
    let linear=sample_linear(in.uv);
    let encoded=select(linear*12.92,1.055*pow(max(linear,vec3f(0.)),vec3f(1./2.4))-0.055,linear>vec3f(0.0031308));
    return vec4f(select(encoded,linear,color_space.x>0.5),1.);
}
