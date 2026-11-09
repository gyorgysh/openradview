// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(4) var surface_cells:texture_2d<u32>;
@group(0) @binding(5) var surface_depth:texture_2d<f32>;
@group(0) @binding(6) var<storage,read_write> failed:atomic<u32>;
// Linear luminance, selected ID, ambient and diffuse weights.
@group(0) @binding(7) var<uniform> surface:vec4f;
@group(0) @binding(8) var surface_normal:texture_2d<f32>;
fn invalid_sample()->vec2f {atomicStore(&failed,1u);return vec2f(0.);}
fn evaluate_surface(pixel:vec2i)->vec2f {
    let cell=textureLoad(surface_cells,pixel,0);
    let depth=textureLoad(surface_depth,pixel,0).r;
    let normal=textureLoad(surface_normal,pixel,0);
    let hit=cell.w!=0u;
    let offset=pixel_ray_offset(pixel,textureDimensions(surface_cells));
    let origin=p.origin.xyz+p.right.xyz*offset.x+p.down.xyz*offset.y;
    var ray=source_interval(origin,p.direction.xyz,offset);
    var background=0.0030352698;
    if hit {
        let dimensions=textureDimensions(source);
        if cell.w!=u32(surface.y) || any(cell.xyz>=dimensions-vec3u(1u)) || !(depth>=-1e20 && depth<=1e20) {
            return invalid_sample();
        }
        let local=origin+p.direction.xyz*depth-vec3f(cell.xyz);
        // This checks the retained candidate's source cell, not mask membership
        // or an authority to edit a smoothed surface.
        if !all(local>=vec3f(-.005)) || !all(local<=vec3f(1.005)) {return invalid_sample();}
        var diffuse=0.;
        if normal.w==1. {
            let magnitude=length(normal.xyz);
            if !(magnitude>=.98 && magnitude<=1.02) {return invalid_sample();}
            diffuse=surface.w*abs(dot(normal.xyz/magnitude,p.light.xyz));
        } else if normal.w!=0. || surface.w!=0. || any(normal.xyz!=vec3f(0.)) {
            // Lit surfaces need a usable normal; never invent one.
            return invalid_sample();
        }
        background=surface.x*(surface.z+diffuse);
        ray.y=min(ray.y,depth);
    } else if any(cell.xyz!=vec3u(0u)) || depth!=0. || any(normal!=vec4f(0.)) {
        return invalid_sample();
    }
    let integrated=integrate_surface_source(origin,ray);
    let result=integrated.x+integrated.y*background;
    if !(result>=0. && result<=1.00001) {return invalid_sample();}
    return vec2f(result,select(0.,integrated.y,hit));
}
