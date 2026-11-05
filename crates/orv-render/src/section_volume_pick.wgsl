// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var output:texture_storage_2d<rgba32float,write>;
@compute @workgroup_size(1,1,1)
fn main() {
    textureStore(output,vec2i(0),evaluate_section(vec2i(p.region.xy)));
}
