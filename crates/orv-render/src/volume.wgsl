// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    origin: vec4f, right: vec4f, down: vec4f, direction: vec4f,
    display: vec4f, transfer: vec4f, sampling: vec4f, region: vec4f,
}
@group(0) @binding(0) var source: texture_3d<f32>;
@group(0) @binding(1) var<uniform> p: Params;
@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm,write>;
@group(0) @binding(3) var validity: texture_3d<u32>;

fn interval(origin:vec3f,direction:vec3f)->vec2f {
    let maximum=vec3f(textureDimensions(source))-1.;
    var near=-1.e30; var far=1.e30;
    for(var axis=0u;axis<3u;axis+=1u) {
        if abs(direction[axis])<1.e-12 {
            if origin[axis]<0. || origin[axis]>maximum[axis] { return vec2f(1.,-1.); }
        } else {
            let a=-origin[axis]/direction[axis];
            let b=(maximum[axis]-origin[axis])/direction[axis];
            near=max(near,min(a,b)); far=min(far,max(a,b));
        }
    }
    return vec2f(near,far);
}
fn scalar(point:vec3f)->f32 {
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3i(floor(q)); let b=min(a+1,maximum); let t=fract(q);
    let c00=mix(textureLoad(source,a,0).r,textureLoad(source,vec3i(b.x,a.y,a.z),0).r,t.x);
    let c10=mix(textureLoad(source,vec3i(a.x,b.y,a.z),0).r,textureLoad(source,vec3i(b.x,b.y,a.z),0).r,t.x);
    let c01=mix(textureLoad(source,vec3i(a.x,a.y,b.z),0).r,textureLoad(source,vec3i(b.x,a.y,b.z),0).r,t.x);
    let c11=mix(textureLoad(source,vec3i(a.x,b.y,b.z),0).r,textureLoad(source,b,0).r,t.x);
    return mix(mix(c00,c10,t.y),mix(c01,c11,t.y),t.z);
}
fn scalar_valid(point:vec3f)->bool {
    if p.sampling.w<0.5 {return true;}
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3i(floor(q));let b=min(a+1,maximum);let t=fract(q);
    for(var z=0u;z<2u;z+=1u) {
        for(var y=0u;y<2u;y+=1u) {
            for(var x=0u;x<2u;x+=1u) {
                let hi=vec3<bool>(x==1u,y==1u,z==1u);
                let w=select(vec3f(1.)-t,t,hi);
                if w.x*w.y*w.z>0. && textureLoad(validity,select(a,b,hi),0).r==0u {return false;}
            }
        }
    }
    return true;
}
fn gray(value:f32)->f32 {
    var g=select(clamp((value-(p.display.y-0.5))/max(p.display.x-1.,0.0001)+0.5,0.,1.),
        select(0.,1.,value>p.display.y-0.5),p.display.x<=1.);
    if p.display.z>0.5 { g=1.-g; }
    return g;
}
fn opacity_weight(value:f32)->f32 {
    let t=(value-p.transfer.x)/(p.transfer.y-p.transfer.x);
    return select(clamp(t,0.,1.),clamp(1.-abs(2.*t-1.),0.,1.),p.transfer.w>0.5);
}
fn encode_rgb(linear:vec3f)->vec3f {
    return select(linear*12.92,1.055*pow(max(linear,vec3f(0.)),vec3f(1./2.4))-0.055,linear>vec3f(0.0031308));
}
fn decode_rgb(encoded:vec3f)->vec3f {
    return select(encoded/12.92,pow((encoded+0.055)/1.055,vec3f(2.4)),encoded>vec3f(0.04045));
}
fn color(point:vec3f)->vec4f {
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3i(floor(q));let b=min(a+1,maximum);let t=fract(q);
    let c00=mix(textureLoad(source,a,0),textureLoad(source,vec3i(b.x,a.y,a.z),0),t.x);
    let c10=mix(textureLoad(source,vec3i(a.x,b.y,a.z),0),textureLoad(source,vec3i(b.x,b.y,a.z),0),t.x);
    let c01=mix(textureLoad(source,vec3i(a.x,a.y,b.z),0),textureLoad(source,vec3i(b.x,a.y,b.z),0),t.x);
    let c11=mix(textureLoad(source,vec3i(a.x,b.y,b.z),0),textureLoad(source,b,0),t.x);
    return mix(mix(c00,c10,t.y),mix(c01,c11,t.y),t.z);
}
fn color_display(linear:vec3f)->vec3f {
    if p.display.z>0.5 {return decode_rgb(vec3f(1.)-encode_rgb(linear));}
    return linear;
}
fn color_ray(origin:vec3f,ray:vec2f)->vec3f {
    let background=decode_rgb(vec3f(10./255.));
    if ray.y<ray.x {return encode_rgb(background);}
    let length=max(ray.y-ray.x,0.);
    let count=u32(clamp(ceil(length/max(p.sampling.x,1.e-30)),1.,p.sampling.y));
    let step=length/f32(count);
    if p.display.w<0.5 {
        var maximum=vec3f(0.);var found=false;
        for(var i=0u;i<=count;i+=1u) {
            let value=color(origin+p.direction.xyz*(ray.x+f32(i)*step));
            if value.a<0.99999 {continue;}
            maximum=max(maximum,value.rgb);found=true;
        }
        if found {return encode_rgb(color_display(maximum));}
        return encode_rgb(background);
    }
    var accumulated=0.;var rgb=vec3f(0.);
    for(var i=0u;i<count;i+=1u) {
        let value=color(origin+p.direction.xyz*(ray.x+(f32(i)+0.5)*step));
        if value.a<0.99999 {continue;}
        let brightness=dot(value.rgb,vec3f(0.2126,0.7152,0.0722))*255.;
        let alpha=1.-exp(-p.transfer.z*opacity_weight(brightness)*step);
        rgb+=(1.-accumulated)*alpha*color_display(value.rgb);
        accumulated+=(1.-accumulated)*alpha;
        if accumulated>=0.995 {break;}
    }
    return encode_rgb(rgb+(1.-accumulated)*background);
}
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) { return; }
    let pixel=vec2u(p.region.xy)+gid.xy;
    let size=textureDimensions(output);
    if any(pixel>=size) { return; }
    let uv=(vec2f(pixel)+0.5)/vec2f(size);
    let origin=p.origin.xyz+p.right.xyz*(uv.x-0.5)+p.down.xyz*(uv.y-0.5);
    let ray=interval(origin,p.direction.xyz);
    if p.sampling.z>0.5 {
        textureStore(output,vec2i(pixel),vec4f(color_ray(origin,ray),1.));return;
    }
    var result=10./255.;
    if ray.y>=ray.x {
        let length=max(ray.y-ray.x,0.);
        let count=u32(clamp(ceil(length/max(p.sampling.x,1.e-30)),1.,p.sampling.y));
        let step=length/f32(count);
        if p.display.w<0.5 {
            var maximum=-1.e30;
            var found=false;
            for(var i=0u;i<=count;i+=1u) {
                let point=origin+p.direction.xyz*(ray.x+f32(i)*step);
                if !scalar_valid(point) {continue;}
                maximum=max(maximum,scalar(point));found=true;
            }
            if found {result=gray(maximum);}
        } else {
            var accumulated=0.; var luminance=0.;
            for(var i=0u;i<count;i+=1u) {
                let point=origin+p.direction.xyz*(ray.x+(f32(i)+0.5)*step);
                if !scalar_valid(point) {continue;}
                let value=scalar(point);
                let alpha=1.-exp(-p.transfer.z*opacity_weight(value)*step);
                luminance+=(1.-accumulated)*alpha*gray(value);
                accumulated+=(1.-accumulated)*alpha;
                if accumulated>=0.995 { break; }
            }
            result=luminance+(1.-accumulated)*(10./255.);
        }
    }
    textureStore(output,vec2i(pixel),vec4f(result,result,result,1.));
}
