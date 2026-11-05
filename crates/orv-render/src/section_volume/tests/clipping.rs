// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn a_coplanar_cut_keeps_the_whole_plane_and_distinguishes_either_side_without_a_tolerance() {
    let (d, q) = device(None);
    let cpu = source([17, 15, 9], 1, false, false);
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu.clone());
    scene.size = [113, 89];
    scene.volume.camera.yaw_degrees = 140.;
    scene.volume.camera.pitch_degrees = 15.;
    scene.material = material(0.);
    let point =
        std::array::from_fn(|a| cpu.center()[a] + [0.12345, -0.897338368742311, 0.34567][a]);
    let grids = [Plane::Axial, Plane::Coronal, Plane::Sagittal]
        .map(|plane| cpu.reformat_grid(plane, point, 64).unwrap());
    let mut reslicer = GpuReslicer::new(gpu).unwrap();
    let images = reslicer
        .render_frames(grids.each_ref().map(Some), || false)
        .unwrap();
    for (slot, axis) in [2, 1, 0].into_iter().enumerate() {
        let mut plane = scene.planes[0].clone().unwrap();
        plane.image = images[slot].clone().unwrap();
        let coordinate = plane.image.grid().geometry().position[axis];
        scene.planes = [Some(plane), None, None];
        scene.volume.clip = None;
        let full = renderer
            .render_rows(&scene, 0, 0..scene.size[1], || false)
            .unwrap()
            .unwrap();
        let expected = pixels(&d, &q, full.texture());
        assert!(verify(&full, &expected).0 > 100);
        for keep_higher in [false, true] {
            // Even the nearest representable f64 coordinate outside the plane
            // is outside; an epsilon must not admit it to conceal boundary noise.
            let outside = if keep_higher {
                coordinate.next_up()
            } else {
                coordinate.next_down()
            };
            let inside = if keep_higher {
                coordinate.next_down()
            } else {
                coordinate.next_up()
            };
            for (edge, retained) in [(coordinate, true), (inside, true), (outside, false)] {
                let mut clip = PhysicalClip {
                    minimum_mm: point.map(|v| v - 100.),
                    maximum_mm: point.map(|v| v + 100.),
                };
                if keep_higher {
                    clip.minimum_mm[axis] = edge;
                } else {
                    clip.maximum_mm[axis] = edge;
                }
                scene.volume.clip = Some(clip);
                assert!(
                    renderer
                        .render_rows(&scene, 0, 0..17, || false)
                        .unwrap()
                        .is_none()
                );
                let image = renderer
                    .render_rows(&scene, 0, 17..scene.size[1], || false)
                    .unwrap()
                    .unwrap();
                let actual = pixels(&d, &q, image.texture());
                if retained {
                    assert!(
                        actual == expected,
                        "axis {axis}, higher {keep_higher}, edge {edge}: {} changed pixels",
                        actual.iter().zip(&expected).filter(|(a, b)| a != b).count()
                    );
                } else {
                    assert!(
                        actual
                            .iter()
                            .all(|p| *p == [0x1a37, 0x1a37, 0x1a37, 0x3c00]),
                        "An outside plane must be absent: axis {axis}, higher {keep_higher}, edge {edge}"
                    );
                }
            }
        }
    }
}
