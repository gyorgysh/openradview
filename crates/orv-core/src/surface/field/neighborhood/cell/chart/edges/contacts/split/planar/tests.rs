// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn disconnected_edge_components_are_refused_while_isolated_nodes_are_retained() {
    let mut parents = std::array::from_fn::<_, NODES, _>(|i| i as u8);
    let mut degrees = [0usize; NODES];
    parents[1] = 0;
    parents[3] = 2;
    degrees[..4].fill(1);
    assert!(matches!(
        connected(&degrees, &mut parents, &|| false),
        Err(SurfaceError::Geometry)
    ));
    parents[2] = 0;
    assert!(connected(&degrees, &mut parents, &|| false).is_ok());
    assert!(matches!(
        connected(&degrees, &mut parents, &|| true),
        Err(SurfaceError::Cancelled)
    ));
    assert!(matches!(
        connected(&[0; NODES], &mut parents, &|| false),
        Err(SurfaceError::Geometry)
    ));
}
