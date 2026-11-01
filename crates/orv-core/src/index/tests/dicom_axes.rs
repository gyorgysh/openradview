// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::acquisition::{
    AxesError, CineError, CineMetadata, DicomAxis, DicomCatalog, SelectionError,
};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use dicom_object::InMemDicomObject as Object;
#[path = "../../../tests/support/dicom_axes.rs"]
mod phantom;

#[test]
fn declared_temporal_and_echo_values_keep_their_ordinals_and_resolve_each_spatial_stack() {
    let temp = Temp::new();
    let path = temp.0.join("axes.dcm");
    phantom::enhanced().write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(study.series.len(), 4);
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    for time in 1..=2 {
        for echo in 1..=2 {
            let index = catalog.resolve(0, [time, echo]).unwrap();
            let axes = catalog.axes(index).unwrap().unwrap();
            assert_eq!(
                axes.value(DicomAxis::TemporalPosition).unwrap().value,
                Some([2., 7.][time as usize - 1])
            );
            assert_eq!(
                axes.value(DicomAxis::EchoTime).unwrap().value,
                Some([4.5, 9.25][echo as usize - 1])
            );
            let series = &study.series[index];
            let expected = [
                [
                    "56724f23e8c9da80fec315bb29bc2b33047f23452ecb489148f3512a55c0a02b",
                    "a43adefb0c58ce6babf8767fca88fcd380144ec5df59f53e5d23a12c008ff7c7",
                ],
                [
                    "b2f2b00158d4b9b8e88a8d9058b63b80a25ac608c06ec8969c66704bc9435e8f",
                    "5c60895c8657c57bf614c80feab17a16b6e14ba64dbb19383adfbc705e629e87",
                ],
            ];
            // Independently calculated established partition bytes, without navigation masking.
            assert_eq!(
                crate::scene::hex(&series.acquisition.unwrap()),
                expected[time as usize - 1][echo as usize - 1]
            );
            assert_eq!(series.frame_count(), 3);
            assert!(series.frames.is_empty());
            for z in 0..3 {
                let frame = series.read_frame(z, || false).unwrap();
                assert_eq!(
                    frame.pixels[0],
                    [2., 7.][time as usize - 1] * 100. + echo as f32 * 10. + z as f32
                );
                assert_eq!(
                    frame.geometry.as_ref().unwrap().position,
                    [0., 0., z as f64 * 3.]
                );
            }
        }
    }
}

fn organization(object: &mut DefaultDicomObject, uid: &str) {
    for tag in [Tag(0x0020, 0x9221), Tag(0x0020, 0x9222)] {
        edit_items(object, tag, |items| {
            for item in items {
                item.put_str(Tag(0x0020, 0x9164), VR::UI, uid);
            }
        });
    }
}
fn edit_items(object: &mut Object, tag: Tag, mut edit: impl FnMut(&mut [Object])) {
    assert!(object.update_value(tag, |value| edit(value.items_mut().unwrap().as_mut_slice())));
}
fn add_pixels(object: &mut DefaultDicomObject, offset: u16) {
    let values = object
        .element(Tag(0x7fe0, 0x0010))
        .unwrap()
        .to_multi_int::<u16>()
        .unwrap();
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(values.into_iter().map(|v| v + offset).collect()),
    ));
}
#[test]
fn organization_series_and_unselected_diffusion_context_isolate_acquisition_families() {
    for mode in 0..3 {
        let temp = Temp::new();
        let mut first = phantom::enhanced();
        let mut second = first.clone();
        second.put_str(Tag(0x0008, 0x0018), VR::UI, "2.25.5150002");
        add_pixels(&mut second, 2000);
        match mode {
            0 => organization(&mut second, "2.25.515002"),
            1 => {
                second.put_str(Tag(0x0020, 0x000e), VR::UI, "2.25.515011");
            }
            _ => {
                for (object, value) in [(&mut first, 500f64), (&mut second, 800.)] {
                    edit_items(object, Tag(0x5200, 0x9230), |items| {
                        for frame in items {
                            frame.put(phantom::sequence(
                                Tag(0x0018, 0x9117),
                                vec![dicom_object::InMemDicomObject::from_element_iter([
                                    DataElement::new(
                                        Tag(0x0018, 0x9087),
                                        VR::FD,
                                        PrimitiveValue::from(value),
                                    ),
                                ])],
                            ));
                        }
                    });
                }
            }
        }
        let paths = [temp.0.join("first.dcm"), temp.0.join("second.dcm")];
        first.write_to_file(&paths[0]).unwrap();
        second.write_to_file(&paths[1]).unwrap();
        let study = index_paths(&paths, || false, |_| {}).unwrap();
        assert_eq!(study.series.len(), 8);
        let catalog = DicomCatalog::prepare(&study, || false).unwrap();
        let a = study
            .series
            .iter()
            .position(|s| s.read_frame(0, || false).unwrap().pixels[0] < 1000.)
            .unwrap();
        let b = study
            .series
            .iter()
            .position(|s| s.read_frame(0, || false).unwrap().pixels[0] > 2000.)
            .unwrap();
        let a = catalog.resolve(a, [2, 2]).unwrap();
        let b = catalog.resolve(b, [2, 2]).unwrap();
        assert_eq!(
            study.series[a].read_frame(0, || false).unwrap().pixels[0],
            720.
        );
        assert_eq!(
            study.series[b].read_frame(0, || false).unwrap().pixels[0],
            2720.
        );
    }
}
#[test]
fn inconsistent_pointed_values_missing_scope_unknown_dimensions_and_duplicates_never_guess() {
    let temp = Temp::new();
    let mut object = phantom::enhanced();
    edit_items(&mut object, Tag(0x5200, 0x9230), |frames| {
        edit_items(&mut frames[0], Tag(0x0020, 0x9111), |items| {
            items[0].put(DataElement::new(
                Tag(0x0020, 0x9128),
                VR::UL,
                PrimitiveValue::from(8u32),
            ));
        });
    });
    let path = temp.0.join("inconsistent.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    assert!(matches!(
        catalog.axes(0),
        Some(Err(AxesError::Inconsistent))
    ));
    for mode in 0..2 {
        let mut object = phantom::enhanced();
        if mode == 0 {
            object.remove_element(Tag(0x0020, 0x9221));
        } else {
            edit_items(&mut object, Tag(0x0020, 0x9222), |items| {
                for descriptor in items.iter_mut().take(2) {
                    descriptor.put(DataElement::new(
                        Tag(0x0020, 0x9165),
                        VR::AT,
                        PrimitiveValue::Tags(vec![Tag(0x0020, 0x0013 + mode as u16)].into()),
                    ));
                }
            });
        }
        let path = temp.0.join(format!("unavailable-{mode}.dcm"));
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let catalog = DicomCatalog::prepare(&study, || false).unwrap();
        assert!(catalog.axes(0).unwrap().is_err());
        assert!(study.series[0].read_frame(0, || false).is_ok());
    }
    let path = temp.0.join("duplicate.dcm");
    phantom::enhanced().write_to_file(&path).unwrap();
    let mut study = index_paths(&[path], || false, |_| {}).unwrap();
    study.series.push(study.series[0].clone());
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    assert_eq!(catalog.resolve(0, [2, 2]), Err(SelectionError::Ambiguous));
}
#[test]
fn missing_pointed_values_are_explicit_and_unknown_dimensions_stay_fixed() {
    let temp = Temp::new();
    let mut object = phantom::enhanced();
    edit_items(&mut object, Tag(0x5200, 0x9230), |items| {
        for frame in items {
            edit_items(frame, Tag(0x0020, 0x9111), |items| {
                items[0].remove_element(Tag(0x0020, 0x9128));
            });
        }
    });
    let path = temp.0.join("missing.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    let index = catalog.resolve(0, [1, 2]).unwrap();
    assert_eq!(
        catalog
            .axes(index)
            .unwrap()
            .unwrap()
            .value(DicomAxis::TemporalPosition)
            .unwrap()
            .value,
        None
    );
    let mut object = phantom::enhanced();
    edit_items(&mut object, Tag(0x0020, 0x9222), |items| {
        items[0].put(DataElement::new(
            Tag(0x0020, 0x9165),
            VR::AT,
            PrimitiveValue::Tags(vec![Tag(0x0020, 0x0013)].into()),
        ));
    });
    let path = temp.0.join("unknown.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    let axes = catalog.axes(0).unwrap().unwrap();
    assert!(axes.other_dimensions());
    assert!(axes.value(DicomAxis::TemporalPosition).is_none());
    let index = catalog.resolve(0, [0, 1]).unwrap();
    assert_eq!(
        study.series[index].read_frame(0, || false).unwrap().pixels[0],
        710.
    );
}

#[test]
fn mixed_partitioned_and_spatially_sorted_frames_refuse_a_source_clock_without_losing_pixels() {
    let temp = Temp::new();
    let mut first = phantom::cine(false);
    let mut second = first.clone();
    second.put_str(Tag(0x0020, 0x0013), VR::IS, "2");
    second.put_str(Tag(0x0008, 0x0018), VR::UI, "2.25.5150002");
    let paths = [temp.0.join("first.dcm"), temp.0.join("second.dcm")];
    first.write_to_file(&paths[0]).unwrap();
    second.write_to_file(&paths[1]).unwrap();
    let study = index_paths(&paths, || false, |_| {}).unwrap();
    assert_eq!(study.series.len(), 1);
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    assert!(matches!(
        catalog.cine(0),
        Some(CineMetadata::Unavailable(CineError::Order))
    ));
    let mut enhanced = phantom::enhanced();
    enhanced.put(DataElement::new(
        Tag(0x0028, 0x0009),
        VR::AT,
        PrimitiveValue::Tags(vec![Tag(0x0018, 0x1063)].into()),
    ));
    enhanced.put_str(Tag(0x0018, 0x1063), VR::DS, "50");
    let path = temp.0.join("partitioned.dcm");
    enhanced.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    assert!(matches!(
        catalog.cine(0),
        Some(CineMetadata::Unavailable(CineError::Order))
    ));
    let mut enhanced = phantom::enhanced();
    let descriptors = enhanced
        .element(Tag(0x0020, 0x9222))
        .unwrap()
        .value()
        .items()
        .unwrap();
    let descriptor = descriptors[2].clone();
    let groups = enhanced
        .element(Tag(0x5200, 0x9230))
        .unwrap()
        .value()
        .items()
        .unwrap();
    let mut selected: Vec<_> = [0usize, 1, 6].map(|index| groups[index].clone()).into();
    for (frame, z) in selected.iter_mut().zip([2u32, 0, 1]) {
        frame.remove_element(Tag(0x0018, 0x9114));
        edit_items(frame, Tag(0x0020, 0x9111), |items| {
            items[0].remove_element(Tag(0x0020, 0x9128));
            items[0].put(DataElement::new(
                Tag(0x0020, 0x9157),
                VR::UL,
                PrimitiveValue::U32(vec![z + 1].into()),
            ));
        });
    }
    enhanced.put(phantom::sequence(Tag(0x0020, 0x9222), vec![descriptor]));
    enhanced.put(phantom::sequence(Tag(0x5200, 0x9230), selected));
    enhanced.put_str(Tag(0x0028, 0x0008), VR::IS, "3");
    enhanced.put(DataElement::new(
        Tag(0x0028, 0x0009),
        VR::AT,
        PrimitiveValue::Tags(vec![Tag(0x0018, 0x1063)].into()),
    ));
    enhanced.put_str(Tag(0x0018, 0x1063), VR::DS, "50");
    enhanced.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16((0u16..576).collect()),
    ));
    let path = temp.0.join("sorted.dcm");
    enhanced.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(study.series.len(), 1);
    assert_eq!(
        study.series[0].read_frame(0, || false).unwrap().frame_index,
        1
    );
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    assert!(matches!(
        catalog.cine(0),
        Some(CineMetadata::Unavailable(CineError::Order))
    ));
    assert!(study.series[0].read_frame(2, || false).is_ok());
    first.put_str(Tag(0x0018, 0x1063), VR::DS, "-1");
    let path = temp.0.join("invalid.dcm");
    first.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    assert!(matches!(
        catalog.cine(0),
        Some(CineMetadata::Unavailable(CineError::Values))
    ));
    assert!(study.series[0].read_frame(0, || false).is_ok());
}

#[test]
fn acquisition_and_timing_catalogs_release_sources_and_cancel_metadata_work() {
    let temp = Temp::new();
    let path = temp.0.join("release.dcm");
    phantom::cine(true).write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let weak = Arc::downgrade(study.series[0].source.as_ref().unwrap());
    let frame = study.series[0].read_frame(0, || false).unwrap();
    let pixels = Arc::downgrade(&frame);
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    assert!(matches!(
        DicomCatalog::prepare(&study, || true),
        Err(LoadError::Cancelled)
    ));
    drop(frame);
    drop(study);
    assert!(weak.upgrade().is_none() && pixels.upgrade().is_none());
    assert!(matches!(catalog.cine(0), Some(CineMetadata::Available(_))));
}
#[test]
fn source_intervals_use_the_pointer_trim_and_original_frame_order() {
    let temp = Temp::new();
    for variable in [false, true] {
        let path = temp.0.join(format!("cine-{variable}.dcm"));
        phantom::cine(variable).write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let catalog = DicomCatalog::prepare(&study, || false).unwrap();
        let CineMetadata::Available(timing) = catalog.cine(0).unwrap() else {
            panic!("valid source timing")
        };
        assert_eq!(timing.trim(), [1, 3]);
        assert_eq!(timing.delay_ms(), Some(7.5));
        assert_eq!(timing.variable(), variable);
        let expected = if variable {
            [0., 0.04, 0.16, 0.22, 0.30]
        } else {
            [0., 0.05, 0.10, 0.15, 0.20]
        };
        for (frame, time) in expected.into_iter().enumerate() {
            assert!((timing.seconds_from_first(frame as u32).unwrap() - time).abs() < 1e-12);
        }
    }
}
