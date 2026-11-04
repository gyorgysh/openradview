// SPDX-License-Identifier: GPL-3.0-or-later
@group(0) @binding(2) var probe:texture_storage_2d<rgba32float,write>;
@compute @workgroup_size(1,1,1)
fn main() {
    let sample=evaluate_surface(vec2i(p.region.xy));
    // Unquantized linear radiance and foreground transmittance from exactly the
    // same pixel function as the completed image, without a per-frame ID copy.
    textureStore(probe,vec2i(0),vec4f(sample,0.,1.));
}
