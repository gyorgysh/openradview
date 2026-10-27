// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object};

fn sequence(tag: Tag, items: Vec<Object>) -> dicom_object::mem::InMemElement {
    DataElement::new(
        tag,
        VR::SQ,
        DicomValue::new_sequence(items, Length::UNDEFINED),
    )
}
fn enhanced(equal_pixels: bool) -> DefaultDicomObject {
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 0, false, false)))
            .unwrap();
    let class = "1.2.840.10008.5.1.4.1.1.2.1";
    object.put_str(Tag(0x0008, 0x0016), VR::UI, class);
    object.update_meta(|meta| meta.media_storage_sop_class_uid = class.into());
    for (tag, value) in [
        (Tag(0x0028, 0x0010), 2u16),
        (Tag(0x0028, 0x0011), 2),
        (Tag(0x0028, 0x0101), 16),
        (Tag(0x0028, 0x0102), 15),
        (Tag(0x0028, 0x0103), 0),
    ] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "6");
    object.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
    object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
    let shared = Object::from_element_iter([
        sequence(
            Tag(0x0020, 0x9116),
            vec![Object::from_element_iter([DataElement::new(
                Tag(0x0020, 0x0037),
                VR::DS,
                "0\\1\\0\\-1\\0\\0",
            )])],
        ),
        sequence(
            Tag(0x0028, 0x9110),
            vec![Object::from_element_iter([DataElement::new(
                Tag(0x0028, 0x0030),
                VR::DS,
                "2\\0.5",
            )])],
        ),
    ]);
    object.put(sequence(Tag(0x5200, 0x9229), vec![shared]));
    let dimensions = [Tag(0x0020, 0x9128), Tag(0x0020, 0x9057)].map(|pointer| {
        Object::from_element_iter([
            DataElement::new(
                Tag(0x0020, 0x9165),
                VR::AT,
                PrimitiveValue::Tags(vec![pointer].into()),
            ),
            DataElement::new(
                Tag(0x0020, 0x9167),
                VR::AT,
                PrimitiveValue::Tags(vec![Tag(0x0020, 0x9111)].into()),
            ),
        ])
    });
    object.put(sequence(Tag(0x0020, 0x9222), dimensions.into()));
    let mut groups = Vec::new();
    let mut pixels = Vec::new();
    for (time, z) in [(1u32, 2u32), (2, 0), (1, 0), (2, 2), (2, 1), (1, 1)] {
        let content = Object::from_element_iter([
            DataElement::new(
                Tag(0x0020, 0x9157),
                VR::UL,
                PrimitiveValue::U32(vec![time, z + 1].into()),
            ),
            DataElement::new(Tag(0x0020, 0x9128), VR::UL, PrimitiveValue::from(time)),
            DataElement::new(Tag(0x0020, 0x9057), VR::UL, PrimitiveValue::from(z + 1)),
        ]);
        let position = Object::from_element_iter([DataElement::new(
            Tag(0x0020, 0x0032),
            VR::DS,
            format!(
                "{}\\{}\\{}",
                10. + 0.25 * z as f64,
                20. + 0.5 * z as f64,
                30. + 3. * z as f64
            ),
        )]);
        groups.push(Object::from_element_iter([
            sequence(Tag(0x0020, 0x9111), vec![content]),
            sequence(Tag(0x0020, 0x9113), vec![position]),
        ]));
        pixels.extend(
            (0..4).map(|i| ((if equal_pixels { 100 } else { time * 100 }) + z * 10 + i) as u16),
        );
    }
    object.put(sequence(Tag(0x5200, 0x9230), groups));
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(pixels.into()),
    ));
    object
}

#[test]
fn enhanced_frames_partition_time_and_reconstruct_explicit_rotated_tilted_geometry() {
    let temp = Temp::new();
    let path = temp.0.join("enhanced.dcm");
    let object = enhanced(false);
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(study.series.len(), 2);
    assert_eq!(study.frame_count(), 6);
    for (part, series) in study.series.iter().enumerate() {
        assert_eq!(series.frame_count(), 3);
        let first = series.read_frame(0, || false).unwrap();
        assert_eq!(
            first
                .geometry
                .as_ref()
                .unwrap()
                .pixel_to_world([1., 1.])
                .unwrap(),
            [8., 20.5, 30.]
        );
        assert_eq!(first.pixels[0], (part as f32 + 1.) * 100.);
        let navigation = crate::navigation::NavigationStack::prepare(series, || false).unwrap();
        assert_eq!(navigation.center(1), Some([9.25, 20.75, 33.]));
        assert_eq!(navigation.nearest([9.25, 20.75, 33.]), Some(1));
        let volume = series.volume(|| false).unwrap();
        let point = volume.voxel_to_world([0.5, 0.5, 1.]);
        assert_eq!(point, [9.25, 20.75, 33.]);
        assert_eq!(volume.sample(point), Some((part as f32 + 1.) * 100. + 11.5));
    }
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    let eager = crate::load::decode(&bytes).unwrap();
    assert!(
        eager.frames.iter().all(|frame| frame.geometry.is_none()),
        "An unpartitioned eager source cannot become a mixed-time volume"
    );
}

#[test]
fn indexed_and_eager_enhanced_scaling_and_frame_voi_follow_source_frame_identity() {
    for shared_transform in [false, true] {
        let mut object = enhanced(false);
        let mut groups = object
            .element(Tag(0x5200, 0x9230))
            .unwrap()
            .items()
            .unwrap()
            .to_vec();
        let transform = |slope: f64| {
            sequence(
                Tag(0x0028, 0x9145),
                vec![Object::from_element_iter([
                    DataElement::new(Tag(0x0028, 0x1053), VR::DS, slope.to_string()),
                    DataElement::new(Tag(0x0028, 0x1052), VR::DS, "-10"),
                    DataElement::new(Tag(0x0028, 0x1054), VR::LO, "HU"),
                ])],
            )
        };
        for (index, group) in groups.iter_mut().enumerate() {
            if !shared_transform {
                group.put(transform((index + 1) as f64 * 0.25));
            }
            group.put(sequence(
                Tag(0x0028, 0x9132),
                vec![Object::from_element_iter([
                    DataElement::new(
                        Tag(0x0028, 0x1050),
                        VR::DS,
                        (index as f64 * 0.5).to_string(),
                    ),
                    DataElement::new(Tag(0x0028, 0x1051), VR::DS, "0.5"),
                    DataElement::new(Tag(0x0028, 0x1056), VR::CS, "LINEAR_EXACT"),
                ])],
            ));
        }
        if shared_transform {
            let mut shared = object
                .element(Tag(0x5200, 0x9229))
                .unwrap()
                .items()
                .unwrap()[0]
                .clone();
            shared.put(transform(0.25));
            object.put(sequence(Tag(0x5200, 0x9229), vec![shared]));
        }
        object.put(sequence(Tag(0x5200, 0x9230), groups));
        let temp = Temp::new();
        let path = temp.0.join("frame-presentation.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let mut bytes = Vec::new();
        object.write_all(&mut bytes).unwrap();
        let eager = crate::load::decode(&bytes).unwrap();
        for series in &study.series {
            for index in 0..series.frame_count() {
                let frame = series.read_frame(index, || false).unwrap();
                let source_index = frame.frame_index as usize;
                let slope = if shared_transform {
                    0.25
                } else {
                    (source_index + 1) as f32 * 0.25
                };
                assert_eq!(
                    frame.pixels[0],
                    [120., 200., 100., 220., 210., 110.][source_index] * slope - 10.
                );
                assert_eq!(frame.pixels, eager.frames[source_index].pixels);
                assert_eq!(
                    frame.default_window,
                    eager.frames[source_index].default_window
                );
                assert_eq!(
                    frame.default_window.function,
                    crate::VoiFunction::LinearExact
                );
                assert_eq!(frame.default_window.center, source_index as f32 * 0.5);
                assert_eq!(frame.default_window.width, 0.5);
                assert_eq!(
                    frame.scalar.as_ref().unwrap().info(),
                    eager.frames[source_index].scalar.as_ref().unwrap().info()
                );
                assert_eq!(
                    frame.scalar.as_ref().unwrap().info().unit,
                    crate::scalar::Unit::Hounsfield
                );
                assert_eq!(
                    series.source.as_ref().unwrap().original_window(index),
                    Some(frame.default_window)
                );
            }
            assert!(series.volume(|| false).is_ok());
        }
    }
}

#[test]
fn acquisition_identity_differs_even_when_pixels_and_geometry_are_identical() {
    let temp = Temp::new();
    let path = temp.0.join("same-values.dcm");
    enhanced(true).write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let hashes: Vec<_> = study
        .series
        .iter()
        .map(|series| crate::scene::series_fingerprint(series, || false).unwrap())
        .collect();
    assert_ne!(hashes[0], hashes[1]);
    for index in 0..3 {
        assert_eq!(
            study.series[0].read_frame(index, || false).unwrap().pixels,
            study.series[1].read_frame(index, || false).unwrap().pixels
        );
    }
}

#[test]
fn incomplete_dimensions_keep_source_browsing_without_fabricating_physical_geometry() {
    let temp = Temp::new();
    let path = temp.0.join("unknown-dimensions.dcm");
    let mut object = enhanced(false);
    object.remove_element(Tag(0x0020, 0x9222));
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(study.frame_count(), 6);
    for series in study.series {
        for index in 0..series.frame_count() {
            assert!(
                series
                    .read_frame(index, || false)
                    .unwrap()
                    .geometry
                    .is_none()
            );
        }
        assert!(series.volume(|| false).is_err());
    }
}

#[test]
fn contradictory_functional_groups_and_frame_counts_are_refused() {
    for variant in 0..3 {
        let mut object = enhanced(false);
        if variant == 0 {
            object.put_str(Tag(0x0028, 0x0008), VR::IS, "5");
        } else if variant == 1 {
            let shared = object
                .element(Tag(0x5200, 0x9229))
                .unwrap()
                .value()
                .items()
                .unwrap()[0]
                .clone();
            let mut frames = object
                .element(Tag(0x5200, 0x9230))
                .unwrap()
                .value()
                .items()
                .unwrap()
                .to_vec();
            frames[0].put(shared.element(Tag(0x0028, 0x9110)).unwrap().clone());
            object.put(sequence(Tag(0x5200, 0x9230), frames));
        } else {
            object.put(sequence(Tag(0x0040, 0x9096), vec![Object::new_empty()]));
        }
        let mut bytes = Vec::new();
        object.write_all(&mut bytes).unwrap();
        assert!(matches!(
            read_header(&mut std::io::Cursor::new(bytes), || false),
            Err(LoadError::Unsupported)
        ));
    }
}

#[test]
fn functional_group_tables_and_identifiers_have_explicit_bounds() {
    let mut object = enhanced(false);
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "65537");
    object.put(sequence(
        Tag(0x5200, 0x9230),
        vec![Object::new_empty(); 65537],
    ));
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    assert!(matches!(
        read_header(&mut std::io::Cursor::new(bytes), || false),
        Err(LoadError::Limit)
    ));
    for value in [
        "1".repeat(66),
        "1|2".into(),
        "1..2".into(),
        "1.02.3".into(),
        ".1".into(),
        "1.2.".into(),
    ] {
        let mut object = enhanced(false);
        object.put_str(Tag(0x0020, 0x000e), VR::UI, value);
        let mut bytes = Vec::new();
        object.write_all(&mut bytes).unwrap();
        assert!(matches!(
            read_header(&mut std::io::Cursor::new(bytes), || false),
            Err(LoadError::Unsupported)
        ));
    }
}

#[test]
fn echo_diffusion_direction_and_component_partition_without_temporal_indices() {
    for variant in 0..4 {
        let mut object = enhanced(true);
        let class = "1.2.840.10008.5.1.4.1.1.4.1";
        object.put_str(Tag(0x0008, 0x0016), VR::UI, class);
        object.put_str(Tag(0x0008, 0x0060), VR::CS, "MR");
        object.update_meta(|meta| meta.media_storage_sop_class_uid = class.into());
        let mut frames = object
            .element(Tag(0x5200, 0x9230))
            .unwrap()
            .items()
            .unwrap()
            .to_vec();
        for frame in &mut frames {
            let mut content =
                frame.element(Tag(0x0020, 0x9111)).unwrap().items().unwrap()[0].clone();
            let time = content
                .element(Tag(0x0020, 0x9128))
                .unwrap()
                .to_int::<u32>()
                .unwrap();
            let z = content
                .element(Tag(0x0020, 0x9057))
                .unwrap()
                .to_int::<u32>()
                .unwrap();
            content.put(DataElement::new(
                Tag(0x0020, 0x9128),
                VR::UL,
                PrimitiveValue::from(1u32),
            ));
            content.put(DataElement::new(
                Tag(0x0020, 0x9157),
                VR::UL,
                PrimitiveValue::U32(vec![1, z].into()),
            ));
            frame.put(sequence(Tag(0x0020, 0x9111), vec![content]));
            let context = match variant {
                0 => sequence(
                    Tag(0x0018, 0x9114),
                    vec![Object::from_element_iter([DataElement::new(
                        Tag(0x0018, 0x9082),
                        VR::FD,
                        PrimitiveValue::from(f64::from(time) * 10.),
                    )])],
                ),
                1 => sequence(
                    Tag(0x0018, 0x9117),
                    vec![Object::from_element_iter([DataElement::new(
                        Tag(0x0018, 0x9087),
                        VR::FD,
                        PrimitiveValue::from(f64::from(time - 1) * 1000.),
                    )])],
                ),
                2 => sequence(
                    Tag(0x0018, 0x9117),
                    vec![Object::from_element_iter([
                        DataElement::new(Tag(0x0018, 0x9087), VR::FD, PrimitiveValue::from(1000.)),
                        sequence(
                            Tag(0x0018, 0x9076),
                            vec![Object::from_element_iter([DataElement::new(
                                Tag(0x0018, 0x9089),
                                VR::FD,
                                PrimitiveValue::F64(
                                    if time == 1 {
                                        vec![1., 0., 0.]
                                    } else {
                                        vec![0., 1., 0.]
                                    }
                                    .into(),
                                ),
                            )])],
                        ),
                    ])],
                ),
                _ => sequence(
                    Tag(0x0018, 0x9226),
                    vec![Object::from_element_iter([
                        DataElement::new(
                            Tag(0x0008, 0x9208),
                            VR::CS,
                            if time == 1 { "MAGNITUDE" } else { "PHASE" },
                        ),
                        DataElement::new(Tag(0x0008, 0x9206), VR::CS, "VOLUME"),
                    ])],
                ),
            };
            frame.put(context);
        }
        if variant == 3 {
            object.put_str(Tag(0x0008, 0x9208), VR::CS, "MIXED");
            object.put_str(Tag(0x0008, 0x9206), VR::CS, "MIXED");
        }
        object.put(sequence(Tag(0x5200, 0x9230), frames));
        let temp = Temp::new();
        let path = temp.0.join("context.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        assert_eq!(study.series.len(), 2, "context {variant}");
        for series in study.series {
            assert_eq!(series.frame_count(), 3);
            assert!(series.volume(|| false).is_ok(), "context {variant}");
        }
    }
}

#[test]
fn classic_echoes_are_separate_and_unresolved_mixed_or_sampled_geometry_stays_2d() {
    let temp = Temp::new();
    let mut paths = Vec::new();
    for echo in [10., 20.] {
        for z in 0..3 {
            let mut object = dicom_object::from_reader(std::io::Cursor::new(support::fixture(
                z as f64, z, false, false,
            )))
            .unwrap();
            object.put_str(Tag(0x0018, 0x0081), VR::DS, echo.to_string());
            let path = temp.0.join(format!("echo-{echo}-{z}.dcm"));
            object.write_to_file(&path).unwrap();
            paths.push(path);
        }
    }
    let study = index_paths(&paths, || false, |_| {}).unwrap();
    assert_eq!(study.series.len(), 2);
    for series in study.series {
        assert_eq!(series.frame_count(), 3);
        assert!(series.volume(|| false).is_ok());
    }
    for (tag, value) in [
        (Tag(0x0008, 0x9208), "MIXED"),
        (Tag(0x0008, 0x9206), "SAMPLED"),
    ] {
        let mut object = enhanced(false);
        object.put_str(tag, VR::CS, value);
        let spatial = crate::dicom_geometry::read(&object, 6, &|| false).unwrap();
        assert!(spatial.frames.iter().all(|frame| frame.geometry.is_none()));
    }
}

#[test]
fn shared_unsupported_mapping_is_refused_without_a_per_frame_table_and_read_cancels() {
    let mut object = enhanced(false);
    assert!(matches!(
        crate::dicom_geometry::read(&object, 6, &|| true),
        Err(LoadError::Cancelled)
    ));
    object.remove_element(Tag(0x5200, 0x9230));
    let mut shared = object
        .element(Tag(0x5200, 0x9229))
        .unwrap()
        .items()
        .unwrap()[0]
        .clone();
    shared.put(sequence(
        Tag(0x0028, 0x9145),
        vec![Object::from_element_iter([sequence(
            Tag(0x0040, 0x9096),
            vec![Object::new_empty()],
        )])],
    ));
    object.put(sequence(Tag(0x5200, 0x9229), vec![shared]));
    assert!(matches!(
        crate::dicom_geometry::read(&object, 6, &|| false),
        Err(LoadError::Unsupported)
    ));
}

#[test]
fn acquisition_partition_count_is_bounded_before_decoding() {
    let mut object = enhanced(false);
    let template = object
        .element(Tag(0x5200, 0x9230))
        .unwrap()
        .items()
        .unwrap()[0]
        .clone();
    let mut frames = Vec::new();
    for time in 1..=4097u32 {
        let mut frame = template.clone();
        frame.put(sequence(
            Tag(0x0020, 0x9111),
            vec![Object::from_element_iter([
                DataElement::new(Tag(0x0020, 0x9128), VR::UL, PrimitiveValue::from(time)),
                DataElement::new(Tag(0x0020, 0x9057), VR::UL, PrimitiveValue::from(1u32)),
                DataElement::new(
                    Tag(0x0020, 0x9157),
                    VR::UL,
                    PrimitiveValue::U32(vec![time, 1].into()),
                ),
            ])],
        ));
        frames.push(frame);
    }
    object.put(sequence(Tag(0x5200, 0x9230), frames));
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "4097");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0u16; 4 * 4097].into()),
    ));
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    assert!(read_header(&mut std::io::Cursor::new(&bytes), || false).is_ok());
    let temp = Temp::new();
    let path = temp.0.join("too-many-acquisitions.dcm");
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        index_paths(&[path], || false, |_| {}),
        Err(LoadError::Limit)
    ));
}
