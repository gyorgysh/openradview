// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(6) var color_source:texture_3d<f32>;
fn source_eligible(cell:vec3i)->bool {
    return textureLoad(color_source,cell,0).a==1.;
}
