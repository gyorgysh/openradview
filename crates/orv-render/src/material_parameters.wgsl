// SPDX-License-Identifier: GPL-3.0-or-later
struct Params {
    origin:vec4f, right:vec4f, down:vec4f, direction:vec4f,
    display:vec4f, transfer:vec4f, sampling:vec4f, region:vec4f, voi:vec4f,
    world_origin:vec4f, world_right:vec4f, world_down:vec4f, world_direction:vec4f,
    clip_minimum:vec4f, clip_maximum:vec4f,
    inverse_x:vec4f, inverse_y:vec4f, inverse_z:vec4f, light:vec4f,
    optics:vec4f, points:array<vec4f,64>,
}
