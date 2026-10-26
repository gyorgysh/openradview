// SPDX-License-Identifier: GPL-3.0-or-later
pub(crate) type V3 = [f64; 3];
pub(crate) fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] + b[i])
}
pub(crate) fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] - b[i])
}
pub(crate) fn mul(a: V3, s: f64) -> V3 {
    a.map(|x| x * s)
}
pub(crate) fn dot(a: V3, b: V3) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub(crate) fn finite(a: V3) -> bool {
    a.iter().all(|v| v.is_finite())
}
