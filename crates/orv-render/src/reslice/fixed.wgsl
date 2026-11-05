// SPDX-License-Identifier: GPL-3.0-or-later
struct Offset {lo:vec4u,hi:vec4u}
struct FixedGrid {origin_lo:vec4u,origin_hi:vec4u,du_lo:vec4u,du_hi:vec4u,dv_lo:vec4u,dv_hi:vec4u,errors:vec4u,offsets:array<Offset,257>}
@group(1) @binding(0) var<uniform> fixed:FixedGrid;
struct FixedSample {value:f32,valid:bool,repair:bool,error:f32}
fn add_fixed(a:vec2u,b:vec2u)->vec2u {
    let lo=a.x+b.x;return vec2u(lo,a.y+b.y+select(0u,1u,lo<a.x));
}
fn times_index(a:vec2u,index:u32)->vec2u {
    let low=(a.x&65535u)*index;let high=(a.x>>16u)*index;
    let lo=low+(high<<16u);
    return vec2u(lo,a.y*index+(high>>16u)+select(0u,1u,lo<low));
}
fn complement_fraction(lo:u32,hi:u32)->vec2u {
    return vec2u(0u-lo,256u-hi-select(0u,1u,lo!=0u));
}
fn fraction_float(v:vec2u)->f32 {return f32(v.y)*(1./256.)+f32(v.x)*(1./1099511627776.);}
fn sample_fixed(pixel:vec2u,sample_index:u32)->FixedSample {
    let maximum=vec3i(textureDimensions(source))-1;
    var base:vec3i;var low:vec3f;var high:vec3f;
    var before:vec3i;var after:vec3i;var errors=fixed.errors.xyz;
    var repair=false;var uncertain=false;
    for(var axis=0u;axis<3u;axis++) {
        var coordinate=add_fixed(vec2u(fixed.origin_lo[axis],fixed.origin_hi[axis]),times_index(vec2u(fixed.du_lo[axis],fixed.du_hi[axis]),pixel.x));
        coordinate=add_fixed(coordinate,times_index(vec2u(fixed.dv_lo[axis],fixed.dv_hi[axis]),pixel.y));
        coordinate=add_fixed(coordinate,vec2u(fixed.offsets[sample_index].lo[axis],fixed.offsets[sample_index].hi[axis]));
        var node=bitcast<i32>(coordinate.y)>>8u;
        var fraction=vec2u(coordinate.x,coordinate.y&255u);
        // Core CPU sampling admits a one-millionth-voxel boundary tolerance.
        // Near that boundary, use its exact f64 rule on the worker.
        let tolerance=1099512u;
        if node<0 || node>=maximum[axis] {
            let distance=select(fraction,complement_fraction(fraction.x,fraction.y),node<0);
            if node< -1 || node>maximum[axis] || distance.y!=0u || distance.x>tolerance {
                let edge=(node== -1 || node==maximum[axis]) && distance.y==0u && distance.x<=tolerance+errors[axis];
                return FixedSample(0.,false,edge,0.);
            }
            if distance.x+errors[axis]>=tolerance {repair=true;}
            if distance.x>errors[axis] {errors[axis]=0u;}
            node=clamp(node+select(0,1,node<0),0,maximum[axis]);fraction=vec2u(0u);
        }
        base[axis]=node;
        high[axis]=fraction_float(fraction);
        low[axis]=fraction_float(complement_fraction(fraction.x,fraction.y));
        before[axis]=node;after[axis]=node+select(0,1,any(fraction!=vec2u(0u)));
        if errors[axis]!=0u {
            if fraction.y==0u && fraction.x<=errors[axis] {before[axis]=node-1;after[axis]=node+1;uncertain=true;}
            let complement=complement_fraction(fraction.x,fraction.y);
            if complement.y==0u && complement.x<=errors[axis] {after[axis]=node+2;uncertain=true;}
        }
    }
    var magnitude=0.;
    if uncertain {
        for(var z=before.z;z<=after.z;z++) {for(var y=before.y;y<=after.y;y++) {for(var x=before.x;x<=after.x;x++) {
            let address=clamp(vec3i(x,y,z),vec3i(0),maximum);
            magnitude=max(magnitude,abs(textureLoad(source,address,0).r));
            if p.origin.w>0.5 && textureLoad(validity,address,0).r==0u {repair=true;}
        }}}
    }
    var value=0.;var absolute=0.;
    for(var z=0u;z<2u;z++) {for(var y=0u;y<2u;y++) {for(var x=0u;x<2u;x++) {
        let upper=vec3<bool>(x==1u,y==1u,z==1u);
        let weights=select(low,high,upper);let weight=weights.x*weights.y*weights.z;
        if weight==0. {continue;}
        let address=min(base+vec3i(vec3u(x,y,z)),maximum);
        if p.origin.w>0.5 && textureLoad(validity,address,0).r==0u {return FixedSample(0.,false,repair,0.);}
        let raw=textureLoad(source,address,0).r;
        magnitude=max(magnitude,abs(raw));value+=raw*weight;absolute+=abs(raw*weight);
    }}}
    // Covers rounded weights, sums, f32 fraction conversion, and possible CPU
    // coordinates across an adjacent cell. Zero fields need no FTZ allowance.
    let position_error=f32(errors.x+errors.y+errors.z)/1099511627776.;
    var error=0.;
    if magnitude!=0. {error=0.00001526*absolute+12.*magnitude*position_error+4.815e-35*(1.+magnitude);}
    return FixedSample(value,true,repair,error);
}
@compute @workgroup_size(8,8,1)
fn frame(@builtin(global_invocation_id) gid:vec3u) {
    if any(gid.xy>=vec2u(p.region.zw)) {return;}
    let pixel=vec2u(p.region.xy)+gid.xy;
    var result=sample_fixed(pixel,0u);
    if p.slab.z>0.5 {
        let total=u32(p.slab.y)+1u;
        var value=0.;var absolute=0.;var error=0.;var repair=false;var count=0u;
        for(var i=0u;i<total;i++) {
            let sample=sample_fixed(pixel,i);repair=repair||sample.repair;
            if !sample.valid {continue;}
            if p.slab.z>2.5 {
                value+=sample.value/f32(total);absolute+=abs(sample.value)/f32(total);error+=sample.error/f32(total);
            } else {
                if count==0u {value=sample.value;}
                else if p.slab.z<1.5 {value=max(value,sample.value);}
                else {value=min(value,sample.value);}
                error=max(error,sample.error);
            }
            count++;
        }
        if count!=0u && p.slab.z>2.5 {
            let scale=f32(total)/f32(count);value*=scale;
            error=(error+(f32(total)+16.)*1.193e-7*absolute)*scale+1.91e-6*abs(value);
        }
        result=FixedSample(value,count!=0u,repair,error);
    }
    let repair=result.repair || (result.valid && result.error>0.0001999*abs(result.value));
    textureStore(output,vec2i(pixel),vec4f(result.value,select(0.,1.,result.valid),select(0.,1.,repair),1.));
}
