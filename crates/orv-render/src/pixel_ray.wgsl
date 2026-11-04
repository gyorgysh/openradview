// SPDX-License-Identifier: GPL-3.0-or-later
// Integer centring preserves an exact zero offset before reciprocal approximation.
fn pixel_ray_offset(pixel:vec2i,size:vec2u)->vec2f {
    return vec2f(pixel*2+vec2i(1)-vec2i(size))/(vec2f(size)*2.);
}
