// SPDX-License-Identifier: GPL-3.0-or-later
// Some backends lower mix to a*(1-t)+b*t, which can perturb a constant
// field and create a spurious lighting normal. Preserve equal endpoints exactly.
fn scalar_mix(a:f32,b:f32,t:f32)->f32 {
    return select(mix(a,b,t),a,a==b);
}
