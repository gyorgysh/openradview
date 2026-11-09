// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(0) var labels:texture_3d<u32>;
fn mask_dimensions()->vec3i {return vec3i(textureDimensions(labels));}
fn mask_member(cell:vec3i)->bool {return textureLoad(labels,cell,0).r==u32(p.control.x);}
