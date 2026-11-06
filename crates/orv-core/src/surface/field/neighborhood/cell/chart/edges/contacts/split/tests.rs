// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn coincident_paths_share_segment_identity_and_keep_direction() {
    let mut directory = [u32::MAX; NODES * NODES];
    let mut count = 0;
    let mut build = |nodes: &[u8]| -> Vec<SplitStep> {
        nodes
            .windows(2)
            .map(|pair| {
                intern(&mut directory, &mut count, [pair[0], pair[1]])
                    .unwrap()
                    .0
            })
            .collect()
    };
    let forward = build(&[3, 7, 9]);
    let reverse = build(&[9, 7, 3]);
    let partial = build(&[7, 9, 10]);
    assert_eq!(
        forward,
        [
            SplitStep {
                segment: 0,
                forward: true
            },
            SplitStep {
                segment: 1,
                forward: true
            }
        ]
    );
    assert_eq!(
        reverse,
        [
            SplitStep {
                segment: 1,
                forward: false
            },
            SplitStep {
                segment: 0,
                forward: false
            }
        ]
    );
    assert_eq!(
        partial,
        [
            SplitStep {
                segment: 1,
                forward: true
            },
            SplitStep {
                segment: 2,
                forward: true
            }
        ]
    );
    assert_eq!(count, 3);
    assert!(matches!(
        intern(&mut directory, &mut count, [3, 3]),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        intern(&mut directory, &mut count, [0, NODES as u8]),
        Err(SurfaceError::Geometry)
    ));
}
