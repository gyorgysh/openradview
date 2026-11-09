// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(0) var packed:texture_3d<u32>;
@group(0) @binding(5) var addresses:texture_3d<u32>;
// Unused vector w lanes retain source dimensions and the original sample maximum.
fn mask_dimensions()->vec3i {return vec3i(vec3f(p.origin.w,p.right.w,p.down.w));}
fn mask_member(cell:vec3i)->bool {
    // Unknown coverage has no positive hit. Negating this predicate alone
    // would not establish a known outside-mask cell.
    let plane=textureLoad(addresses,vec3i(0,0,cell.z),0).r;
    if plane==0xffffffffu {return false;}
    let binary=p.control.w<1.5;
    let shift=select(2u,5u,binary);
    let word=textureLoad(packed,vec3i(cell.x>>shift,cell.y,i32(plane)),0).r;
    if binary {return (word&(1u<<(u32(cell.x)&31u)))!=0u;}
    let value=(word>>((u32(cell.x)&3u)*8u))&255u;
    return value>0u && value*2u>=u32(p.direction.w);
}
