// SPDX-License-Identifier: GPL-3.0-or-later
//! Display axes only. Source geometry and physical coordinates stay unchanged.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DisplayTransform {
    /// Clockwise quarter turns, followed by flips in the displayed axes.
    pub quarter_turns: u8,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}
impl DisplayTransform {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
    pub fn is_valid(&self) -> bool {
        self.quarter_turns < 4
    }
    pub fn swaps_axes(self) -> bool {
        self.quarter_turns % 2 == 1
    }
    pub fn aspect(self, source: f64) -> f64 {
        if self.swaps_axes() {
            1. / source
        } else {
            source
        }
    }
    pub fn apply(self, point: [f64; 2]) -> [f64; 2] {
        let [x, y] = point;
        let [x, y] = match self.quarter_turns % 4 {
            0 => [x, y],
            1 => [-y, x],
            2 => [-x, -y],
            _ => [y, -x],
        };
        [
            if self.flip_horizontal { -x } else { x },
            if self.flip_vertical { -y } else { y },
        ]
    }
    pub fn inverse(self, point: [f64; 2]) -> [f64; 2] {
        let [x, y] = point;
        let x = if self.flip_horizontal { -x } else { x };
        let y = if self.flip_vertical { -y } else { y };
        match self.quarter_turns % 4 {
            0 => [x, y],
            1 => [y, -x],
            2 => [-x, -y],
            _ => [-y, x],
        }
    }
    /// Apply a clockwise rotation to the current displayed image, including flips.
    pub fn rotate_clockwise(&mut self) {
        self.quarter_turns =
            self.quarter_turns
                .wrapping_add(if self.flip_horizontal ^ self.flip_vertical {
                    3
                } else {
                    1
                })
                % 4;
    }
    pub fn rotate_counterclockwise(&mut self) {
        self.quarter_turns =
            self.quarter_turns
                .wrapping_add(if self.flip_horizontal ^ self.flip_vertical {
                    1
                } else {
                    3
                })
                % 4;
    }
    /// Source left/right/top/bottom edge index for each displayed edge.
    pub fn edge_order(self) -> [usize; 4] {
        [[-1., 0.], [1., 0.], [0., -1.], [0., 1.]].map(|edge| {
            let [x, y] = self.inverse(edge);
            if x < 0. {
                0
            } else if x > 0. {
                1
            } else if y < 0. {
                2
            } else {
                3
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quarter_turns_and_display_flips_have_independent_inverses_and_rotation_controls() {
        for quarter_turns in 0..4 {
            for flip_horizontal in [false, true] {
                for flip_vertical in [false, true] {
                    let transform = DisplayTransform {
                        quarter_turns,
                        flip_horizontal,
                        flip_vertical,
                    };
                    let point = [0.125, -0.375];
                    assert_eq!(transform.inverse(transform.apply(point)), point);
                    let displayed = transform.apply(point);
                    let mut clockwise = transform;
                    clockwise.rotate_clockwise();
                    assert_eq!(clockwise.apply(point), [-displayed[1], displayed[0]]);
                    clockwise.rotate_counterclockwise();
                    assert_eq!(clockwise, transform);
                    assert_eq!(
                        transform.aspect(2.),
                        if quarter_turns % 2 == 0 { 2. } else { 0.5 }
                    );
                    let mut edges = transform.edge_order();
                    edges.sort();
                    assert_eq!(edges, [0, 1, 2, 3]);
                }
            }
        }
        let clockwise = DisplayTransform {
            quarter_turns: 1,
            ..Default::default()
        };
        assert_eq!(clockwise.apply([-0.5, -0.5]), [0.5, -0.5]);
        assert_eq!(clockwise.edge_order(), [3, 2, 0, 1]);
        assert!(
            !DisplayTransform {
                quarter_turns: 4,
                ..Default::default()
            }
            .is_valid()
        );
    }
}
