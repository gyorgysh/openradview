// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::index::{SegmentationError, read_segmentation};
use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object};
use std::io::Cursor;
const MASK_CLASS: &str = "1.2.840.10008.5.1.4.1.1.66.4";
const SOURCE_CLASS: &str = "1.2.840.10008.5.1.4.1.1.2";
const REFERENCE: &str = "2.25.3141592653589793238";
fn seq(tag: Tag, items: Vec<Object>) -> dicom_object::mem::InMemElement {
    DataElement::new(
        tag,
        VR::SQ,
        DicomValue::new_sequence(items, Length::UNDEFINED),
    )
}
fn ds(tag: Tag, value: &str) -> dicom_object::mem::InMemElement {
    DataElement::new(tag, VR::DS, value)
}
fn us(tag: Tag, value: u16) -> dicom_object::mem::InMemElement {
    DataElement::new(tag, VR::US, PrimitiveValue::from(value))
}
fn sources(temp: &Temp) -> crate::study::Study {
    let mut paths = Vec::new();
    for (z, instance) in [(4., 1), (0., 9), (2., 5)] {
        let mut source =
            dicom_object::from_reader(Cursor::new(support::fixture(z, instance, false, false)))
                .unwrap();
        source.put(us(Tag(0x28, 0x11), 3));
        let pixels = if z == 0. {
            vec![20u16, u16::MAX, 20, 20, 20, 20]
        } else {
            vec![20u16; 6]
        };
        source.put(DataElement::new(
            Tag(0x7fe0, 0x10),
            VR::OW,
            PrimitiveValue::U16(pixels.into()),
        ));
        if z == 0. {
            source.put(DataElement::new(
                Tag(0x28, 0x120),
                VR::SS,
                PrimitiveValue::from(-1i16),
            ));
        }
        let path = temp.0.join(format!("source-{instance}.dcm"));
        source.write_to_file(&path).unwrap();
        paths.push(path);
    }
    index_paths(&paths, || false, |_| {}).unwrap()
}
fn frame(number: u16, z: f64, instance: i32) -> Object {
    let reference = Object::from_element_iter([
        DataElement::new(Tag(8, 0x1150), VR::UI, SOURCE_CLASS),
        DataElement::new(Tag(8, 0x1155), VR::UI, format!("2.25.314159{instance}")),
        DataElement::new(Tag(0x28, 0x135a), VR::CS, "YES"),
    ]);
    Object::from_element_iter([
        seq(
            Tag(0x62, 0xa),
            vec![Object::from_element_iter([us(Tag(0x62, 0xb), number)])],
        ),
        seq(
            Tag(0x20, 0x9113),
            vec![Object::from_element_iter([ds(
                Tag(0x20, 0x32),
                &format!("0\\0\\{z}"),
            )])],
        ),
        seq(
            Tag(8, 0x9124),
            vec![Object::from_element_iter([seq(
                Tag(8, 0x2112),
                vec![reference],
            )])],
        ),
    ])
}
fn mask() -> DefaultDicomObject {
    let mut object =
        dicom_object::from_reader(Cursor::new(support::fixture(0., 777, false, false))).unwrap();
    object.put_str(Tag(8, 0x16), VR::UI, MASK_CLASS);
    object.update_meta(|meta| meta.media_storage_sop_class_uid = MASK_CLASS.into());
    object.put_str(Tag(8, 0x18), VR::UI, "2.25.2026110101");
    object.update_meta(|meta| meta.media_storage_sop_instance_uid = "2.25.2026110101".into());
    object.put_str(Tag(8, 0x60), VR::CS, "SEG");
    object.put_str(Tag(0x62, 1), VR::CS, "BINARY");
    object.put_str(Tag(0x28, 8), VR::IS, "4");
    object.put_str(Tag(0x62, 0x13), VR::CS, "YES");
    for (tag, value) in [
        (0x10, 2),
        (0x11, 3),
        (2, 1),
        (0x100, 1),
        (0x101, 1),
        (0x102, 0),
        (0x103, 0),
    ] {
        object.put(us(Tag(0x28, tag), value));
    }
    object.put_str(Tag(0x28, 4), VR::CS, "MONOCHROME2");
    object.put_str(Tag(0x20, 0x52), VR::UI, REFERENCE);
    object.put(seq(
        Tag(0x62, 2),
        [9u16, 2]
            .map(|number| {
                Object::from_element_iter([
                    us(Tag(0x62, 4), number),
                    DataElement::new(
                        Tag(0x62, 5),
                        VR::LO,
                        "Generated label, kept out of the public mask model",
                    ),
                    DataElement::new(Tag(0x62, 8), VR::CS, "MANUAL"),
                ])
            })
            .into(),
    ));
    object.put(seq(
        Tag(0x5200, 0x9229),
        vec![Object::from_element_iter([
            seq(
                Tag(0x20, 0x9116),
                vec![Object::from_element_iter([ds(
                    Tag(0x20, 0x37),
                    "1\\0\\0\\0\\1\\0",
                )])],
            ),
            seq(
                Tag(0x28, 0x9110),
                vec![Object::from_element_iter([ds(Tag(0x28, 0x30), "2\\1")])],
            ),
        ])],
    ));
    object.put(seq(
        Tag(0x5200, 0x9230),
        vec![
            frame(9, 4., 1),
            frame(2, 0., 9),
            frame(9, 0., 9),
            frame(2, 2., 5),
        ],
    ));
    object.put(DataElement::new(
        Tag(0x7fe0, 0x10),
        VR::OB,
        PrimitiveValue::U8(vec![0xa9, 0xb2, 0xd0, 0xff].into()),
    ));
    object
}
fn write_mask(temp: &Temp, object: &DefaultDicomObject) -> PathBuf {
    let path = temp.0.join("generated-masks.dcm");
    object.write_to_file(&path).unwrap();
    path
}
#[test]
fn native_little_endian_masks_support_shared_single_frame_macros() {
    let temp = Temp::new();
    let study = sources(&temp);
    for syntax in ["1.2.840.10008.1.2", "1.2.840.10008.1.2.1"] {
        let mut object = mask();
        object.update_meta(|meta| meta.transfer_syntax = syntax.into());
        object.put_str(Tag(0x28, 8), VR::IS, "1");
        let mut shared = object
            .element(Tag(0x5200, 0x9229))
            .unwrap()
            .value()
            .items()
            .unwrap()[0]
            .clone();
        for element in frame(9, 4., 1) {
            shared.put(element);
        }
        object.put(seq(Tag(0x5200, 0x9229), vec![shared]));
        object.remove_element(Tag(0x5200, 0x9230));
        object.put(DataElement::new(
            Tag(0x7fe0, 0x10),
            VR::OB,
            PrimitiveValue::U8(vec![0xa9, 0xff].into()),
        ));
        let path = write_mask(&temp, &object);
        let masks = read_segmentation(&path, &study.series[0], || false).unwrap();
        assert_eq!(
            masks
                .infos()
                .map(|info| (info.number, info.positive_cells, info.supplied_planes))
                .collect::<Vec<_>>(),
            [(9, 3, 1), (2, 0, 0)]
        );
        assert_eq!(masks.value(9, [0, 0, 2]), Some(true));
        assert_eq!(masks.value(9, [0, 0, 0]), None);
    }
}
#[test]
fn replacing_the_selected_mask_file_during_reading_refuses_the_result() {
    let temp = Temp::new();
    let study = sources(&temp);
    let path = write_mask(&temp, &mask());
    let checks = std::cell::Cell::new(0usize);
    read_segmentation(&path, &study.series[0], || {
        checks.set(checks.get() + 1);
        false
    })
    .unwrap();
    let midpoint = checks.get() / 2;
    let replacement = temp.0.join("replacement.dcm");
    fs::copy(&path, &replacement).unwrap();
    checks.set(0);
    let result = read_segmentation(&path, &study.series[0], || {
        checks.set(checks.get() + 1);
        if checks.get() == midpoint {
            fs::rename(&replacement, &path).unwrap();
        }
        false
    });
    assert!(matches!(result, Err(SegmentationError::Changed)));
    assert!(!replacement.exists());
}
#[test]
fn binary_masks_resolve_explicit_sources_and_keep_sparse_overlapping_bits_on_the_exact_volume() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    let volume = series.volume(|| false).unwrap();
    let path = write_mask(&temp, &mask());
    let masks = read_segmentation(&path, series, || false).unwrap();
    assert!(Arc::ptr_eq(&masks.source_arc(), &volume));
    assert_eq!(
        masks
            .infos()
            .map(|i| (
                i.number,
                i.positive_cells,
                i.valid_source_cells,
                i.supplied_planes
            ))
            .collect::<Vec<_>>(),
        [(9, 6, 5, 2), (2, 5, 4, 2)]
    );
    assert_eq!(masks.overlap_cells(), 2);
    assert_eq!(masks.value(9, [1, 0, 0]), Some(true));
    assert_eq!(masks.value(9, [2, 0, 1]), None);
    assert_eq!(masks.value(2, [1, 1, 1]), Some(true));
    assert_eq!(masks.value(9, [2, 1, 2]), Some(true));
    assert_eq!(volume.voxel_volume_mm3(), 4.);
    let ordinary = index_paths_detailed(&[path], || false, |_| {}).unwrap_err();
    assert_eq!(
        ordinary
            .report
            .count(crate::import_report::SkipReason::UnsupportedObject),
        1
    );
}
#[test]
fn identical_numeric_geometry_cannot_replace_missing_or_wrong_source_references() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    for wrong in ["2.25.9999", "2.25.3141595"] {
        let mut object = mask();
        let mut frames = object
            .element(Tag(0x5200, 0x9230))
            .unwrap()
            .value()
            .items()
            .unwrap()
            .to_vec();
        let reference = Object::from_element_iter([
            DataElement::new(Tag(8, 0x1150), VR::UI, SOURCE_CLASS),
            DataElement::new(Tag(8, 0x1155), VR::UI, wrong),
        ]);
        frames[0].put(seq(
            Tag(8, 0x9124),
            vec![Object::from_element_iter([seq(
                Tag(8, 0x2112),
                vec![reference],
            )])],
        ));
        object.put(seq(Tag(0x5200, 0x9230), frames));
        let error = read_segmentation(&write_mask(&temp, &object), series, || false)
            .err()
            .unwrap();
        assert!(matches!(
            error,
            SegmentationError::Association | SegmentationError::Geometry
        ));
        for text in [wrong, "Generated", "source-", "2.25."] {
            assert!(!error.to_string().contains(text));
        }
    }
    let mut object = mask();
    object.put_str(Tag(0x20, 0x52), VR::UI, "2.25.88888");
    assert_eq!(
        read_segmentation(&write_mask(&temp, &object), series, || false).err(),
        Some(SegmentationError::Geometry)
    );
}
#[test]
fn malformed_ids_frames_geometry_payloads_and_other_kinds_are_refused() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    for case in 0..11 {
        let mut object = mask();
        match case {
            0 => {
                object.put_str(Tag(0x62, 1), VR::CS, "FRACTIONAL");
            }
            1 => {
                object.put_str(Tag(0x62, 0x13), VR::CS, "NO");
            }
            2 => {
                object.put(us(Tag(0x28, 0x100), 8));
            }
            3 => {
                object.put_str(Tag(0x28, 8), VR::IS, "3");
            }
            4 => {
                object.put(seq(
                    Tag(0x62, 2),
                    vec![
                        Object::from_element_iter([us(Tag(0x62, 4), 2)]),
                        Object::from_element_iter([us(Tag(0x62, 4), 2)]),
                    ],
                ));
            }
            5 => {
                let mut frames = object
                    .element(Tag(0x5200, 0x9230))
                    .unwrap()
                    .value()
                    .items()
                    .unwrap()
                    .to_vec();
                frames[2] = frames[0].clone();
                object.put(seq(Tag(0x5200, 0x9230), frames));
            }
            6 => {
                let mut frames = object
                    .element(Tag(0x5200, 0x9230))
                    .unwrap()
                    .value()
                    .items()
                    .unwrap()
                    .to_vec();
                frames[0].remove_element(Tag(8, 0x9124));
                object.put(seq(Tag(0x5200, 0x9230), frames));
            }
            7 => {
                object.put(DataElement::new(
                    Tag(0x7fe0, 0x10),
                    VR::OB,
                    PrimitiveValue::U8(vec![0, 0].into()),
                ));
            }
            8 => {
                object.put(DataElement::new(
                    Tag(0x28, 0x11),
                    VR::US,
                    PrimitiveValue::U16(vec![3, 3].into()),
                ));
            }
            9 => {
                object.put_str(Tag(0x20, 0x9161), VR::UI, "2.25.2026110199");
            }
            _ => {
                let mut shared = object
                    .element(Tag(0x5200, 0x9229))
                    .unwrap()
                    .value()
                    .items()
                    .unwrap()[0]
                    .clone();
                shared.put(seq(
                    Tag(0x20, 0x9113),
                    vec![Object::from_element_iter([ds(Tag(0x20, 0x32), "0\\0\\4")])],
                ));
                object.put(seq(Tag(0x5200, 0x9229), vec![shared]));
            }
        }
        assert!(
            read_segmentation(&write_mask(&temp, &object), series, || false).is_err(),
            "case {case}"
        );
    }
    let path = write_mask(&temp, &mask());
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend([0, 0]);
    fs::write(&path, bytes).unwrap();
    assert!(read_segmentation(&path, series, || false).is_err());
}
#[test]
fn changed_backing_sources_are_refused_even_while_the_original_decoded_volume_is_alive() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    let volume = series.volume(|| false).unwrap();
    let path = write_mask(&temp, &mask());
    fs::write(temp.0.join("source-1.dcm"), b"changed generated input").unwrap();
    assert_eq!(
        read_segmentation(&path, series, || false).err(),
        Some(SegmentationError::Changed)
    );
    assert_eq!(volume.dimensions(), [3, 2, 3]);
}
#[test]
fn cancelling_during_parsing_or_association_never_returns_a_partial_mask_set() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    let path = write_mask(&temp, &mask());
    for after in [0, 1, 4, 20, 60] {
        let calls = std::cell::Cell::new(0);
        let result = read_segmentation(&path, series, || {
            let n = calls.get();
            calls.set(n + 1);
            n >= after
        });
        assert_eq!(result.err(), Some(SegmentationError::Cancelled));
    }
}

#[test]
fn unsupported_mask_syntax_is_rejected_before_any_dataset_values_are_read() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    for syntax in [
        "1.2.840.10008.1.2.1.99",
        "1.2.840.10008.1.2.2",
        "1.2.840.10008.1.2.5",
    ] {
        let mut bytes = vec![0; 128];
        bytes.extend(b"DICM");
        dicom_object::meta::FileMetaTableBuilder::new()
            .media_storage_sop_class_uid(MASK_CLASS)
            .media_storage_sop_instance_uid("2.25.2026110101")
            .transfer_syntax(syntax)
            .build()
            .unwrap()
            .write(&mut bytes)
            .unwrap();
        bytes.extend(
            b"Deliberately not a dataset; syntax rejection precedes any parsing or inflation.",
        );
        let path = temp.0.join("unsupported-mask.dcm");
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            read_segmentation(&path, series, || false).err(),
            Some(SegmentationError::Encoding)
        );
    }
}

#[test]
fn reversed_tilted_source_planes_are_associated_by_reference_and_pose() {
    let temp = Temp::new();
    drop(sources(&temp));
    for (z, instance) in [(4., 1), (0., 9), (2., 5)] {
        let path = temp.0.join(format!("source-{instance}.dcm"));
        let mut object = dicom_object::open_file(&path).unwrap();
        object.put_str(Tag(0x20, 0x32), VR::DS, format!("{}\\0\\{}", z * 0.125, -z));
        object.put_str(Tag(0x20, 0x37), VR::DS, "1\\0\\0\\0\\-1\\0");
        object.write_to_file(&path).unwrap();
    }
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    let mut object = mask();
    let mut shared = object
        .element(Tag(0x5200, 0x9229))
        .unwrap()
        .value()
        .items()
        .unwrap()[0]
        .clone();
    shared.put(seq(
        Tag(0x20, 0x9116),
        vec![Object::from_element_iter([ds(
            Tag(0x20, 0x37),
            "1\\0\\0\\0\\-1\\0",
        )])],
    ));
    object.put(seq(Tag(0x5200, 0x9229), vec![shared]));
    let mut frames = object
        .element(Tag(0x5200, 0x9230))
        .unwrap()
        .value()
        .items()
        .unwrap()
        .to_vec();
    for (frame, z) in frames.iter_mut().zip([4., 0., 0., 2.]) {
        frame.put(seq(
            Tag(0x20, 0x9113),
            vec![Object::from_element_iter([ds(
                Tag(0x20, 0x32),
                &format!("{}\\0\\{}", z * 0.125, -z),
            )])],
        ));
    }
    object.put(seq(Tag(0x5200, 0x9230), frames));
    let masks = read_segmentation(&write_mask(&temp, &object), &study.series[0], || false).unwrap();
    assert_eq!(masks.value(9, [2, 1, 2]), Some(true));
    assert_eq!(masks.value(2, [1, 1, 1]), Some(true));
    assert_eq!(
        masks.source_arc().voxel_to_world([0., 0., 2.]),
        [0.5, 0., -4.]
    );
    assert_eq!(masks.source_arc().voxel_volume_mm3(), 4.);
    assert_eq!(masks.overlap_cells(), 2);
}
fn multiframe_source(temp: &Temp) -> crate::study::Study {
    let mut object =
        dicom_object::from_reader(Cursor::new(support::fixture(0., 9, false, false))).unwrap();
    let class = "1.2.840.10008.5.1.4.1.1.2.1";
    object.put_str(Tag(8, 0x16), VR::UI, class);
    object.update_meta(|meta| meta.media_storage_sop_class_uid = class.into());
    object.put(us(Tag(0x28, 0x11), 3));
    object.put_str(Tag(0x28, 8), VR::IS, "3");
    let mut pixels = vec![20u16; 18];
    pixels[1] = u16::MAX;
    object.put(DataElement::new(
        Tag(0x7fe0, 0x10),
        VR::OW,
        PrimitiveValue::U16(pixels.into()),
    ));
    object.put(DataElement::new(
        Tag(0x28, 0x120),
        VR::SS,
        PrimitiveValue::from(-1i16),
    ));
    let shared = mask()
        .element(Tag(0x5200, 0x9229))
        .unwrap()
        .value()
        .items()
        .unwrap()[0]
        .clone();
    object.put(seq(Tag(0x5200, 0x9229), vec![shared]));
    object.put(seq(
        Tag(0x20, 0x9221),
        vec![Object::from_element_iter([DataElement::new(
            Tag(0x20, 0x9164),
            VR::UI,
            "2.25.2026110188",
        )])],
    ));
    object.put(seq(
        Tag(0x20, 0x9222),
        vec![Object::from_element_iter([
            DataElement::new(Tag(0x20, 0x9164), VR::UI, "2.25.2026110188"),
            DataElement::new(
                Tag(0x20, 0x9165),
                VR::AT,
                PrimitiveValue::Tags(vec![Tag(0x20, 0x9057)].into()),
            ),
            DataElement::new(
                Tag(0x20, 0x9167),
                VR::AT,
                PrimitiveValue::Tags(vec![Tag(0x20, 0x9111)].into()),
            ),
        ])],
    ));
    object.put(seq(
        Tag(0x5200, 0x9230),
        (0..3u32)
            .map(|z| {
                Object::from_element_iter([
                    seq(
                        Tag(0x20, 0x9113),
                        vec![Object::from_element_iter([ds(
                            Tag(0x20, 0x32),
                            &format!("0\\0\\{}", z * 2),
                        )])],
                    ),
                    seq(
                        Tag(0x20, 0x9111),
                        vec![Object::from_element_iter([
                            DataElement::new(
                                Tag(0x20, 0x9157),
                                VR::UL,
                                PrimitiveValue::from(z + 1),
                            ),
                            DataElement::new(
                                Tag(0x20, 0x9057),
                                VR::UL,
                                PrimitiveValue::from(z + 1),
                            ),
                        ])],
                    ),
                ])
            })
            .collect(),
    ));
    let path = temp.0.join("enhanced-source.dcm");
    object.write_to_file(&path).unwrap();
    index_paths(&[path], || false, |_| {}).unwrap()
}
fn multiframe_mask(first: Option<&str>) -> DefaultDicomObject {
    let mut object = mask();
    let mut frames = object
        .element(Tag(0x5200, 0x9230))
        .unwrap()
        .value()
        .items()
        .unwrap()
        .to_vec();
    for (i, (frame, number)) in frames.iter_mut().zip([3, 1, 1, 2]).enumerate() {
        let mut reference = Object::from_element_iter([
            DataElement::new(Tag(8, 0x1150), VR::UI, "1.2.840.10008.5.1.4.1.1.2.1"),
            DataElement::new(Tag(8, 0x1155), VR::UI, "2.25.3141599"),
        ]);
        if i > 0 {
            reference.put_str(Tag(8, 0x1160), VR::IS, number.to_string());
        } else if let Some(value) = first {
            reference.put_str(Tag(8, 0x1160), VR::IS, value);
        }
        frame.put(seq(
            Tag(8, 0x9124),
            vec![Object::from_element_iter([seq(
                Tag(8, 0x2112),
                vec![reference],
            )])],
        ));
    }
    object.put(seq(Tag(0x5200, 0x9230), frames));
    object
}
#[test]
fn multiframe_sources_require_one_explicit_original_frame_number() {
    let temp = Temp::new();
    let study = multiframe_source(&temp);
    let series = &study.series[0];
    assert_eq!(series.frame_count(), 3);
    let masks = read_segmentation(
        &write_mask(&temp, &multiframe_mask(Some("3"))),
        series,
        || false,
    )
    .unwrap();
    assert_eq!(
        masks
            .infos()
            .map(|i| (i.number, i.positive_cells, i.valid_source_cells))
            .collect::<Vec<_>>(),
        [(9, 6, 5), (2, 5, 4)]
    );
    assert_eq!(masks.value(9, [2, 1, 2]), Some(true));
    for first in [None, Some("0"), Some("4"), Some("1\\3")] {
        assert_eq!(
            read_segmentation(&write_mask(&temp, &multiframe_mask(first)), series, || {
                false
            })
            .err(),
            Some(SegmentationError::Association)
        );
    }
    assert_eq!(
        read_segmentation(
            &write_mask(&temp, &multiframe_mask(Some("1"))),
            series,
            || false
        )
        .err(),
        Some(SegmentationError::Geometry)
    );
}
fn fractional_mask(values: Vec<u8>) -> DefaultDicomObject {
    let mut object = mask();
    object.put_str(Tag(0x62, 1), VR::CS, "FRACTIONAL");
    object.put_str(Tag(0x62, 0x10), VR::CS, "PROBABILITY");
    object.put(us(Tag(0x62, 0xe), 200));
    for (tag, value) in [(0x100, 8), (0x101, 8), (0x102, 7)] {
        object.put(us(Tag(0x28, tag), value));
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x10),
        VR::OB,
        PrimitiveValue::U8(values.into()),
    ));
    object
}
#[test]
fn fractional_masks_become_positive_at_half_the_declared_maximum() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    // Frames: (9, z4), (2, z0), (9, z0), (2, z2); threshold 100 of 200.
    let values = [[0, 99, 100, 200, 1, 150], [0; 6], [100; 6], [50; 6]].concat();
    let masks = read_segmentation(
        &write_mask(&temp, &fractional_mask(values.clone())),
        series,
        || false,
    )
    .unwrap();
    assert_eq!(
        masks
            .infos()
            .map(|i| (
                i.number,
                i.positive_cells,
                i.valid_source_cells,
                i.supplied_planes
            ))
            .collect::<Vec<_>>(),
        [(9, 9, 8, 2), (2, 0, 0, 2)]
    );
    assert_eq!(masks.overlap_cells(), 0);
    assert_eq!(masks.value(9, [0, 0, 2]), Some(false));
    assert_eq!(masks.value(9, [2, 0, 2]), Some(true));
    assert_eq!(masks.value(9, [1, 0, 2]), Some(false));
    assert_eq!(masks.value(2, [0, 0, 1]), Some(false));
    let mut above = values.clone();
    above[7] = 201;
    assert!(matches!(
        read_segmentation(&write_mask(&temp, &fractional_mask(above)), series, || {
            false
        }),
        Err(SegmentationError::Mask(_))
    ));
    for case in 0..3 {
        let mut object = fractional_mask(values.clone());
        match case {
            0 => {
                object.put(us(Tag(0x62, 0xe), 0));
            }
            1 => {
                object.put_str(Tag(0x62, 0x10), VR::CS, "OTHER");
            }
            _ => {
                object.put(us(Tag(0x28, 0x100), 16));
            }
        }
        assert!(
            read_segmentation(&write_mask(&temp, &object), series, || false).is_err(),
            "case {case}"
        );
    }
}
const LABELMAP_CLASS: &str = "1.2.840.10008.5.1.4.1.1.66.7";
fn labelmap(values: Vec<u16>, eight_bit: bool) -> DefaultDicomObject {
    let mut object = mask();
    object.put_str(Tag(8, 0x16), VR::UI, LABELMAP_CLASS);
    object.update_meta(|meta| meta.media_storage_sop_class_uid = LABELMAP_CLASS.into());
    object.put_str(Tag(0x62, 1), VR::CS, "LABELMAP");
    object.put_str(Tag(0x28, 8), VR::IS, "3");
    object.remove_element(Tag(0x62, 0x13));
    let frames = [(4., 1), (0., 9), (2., 5)]
        .map(|(z, instance)| {
            let mut item = frame(9, z, instance);
            item.remove_element(Tag(0x62, 0xa));
            item
        })
        .into();
    object.put(seq(Tag(0x5200, 0x9230), frames));
    let bits = if eight_bit { 8 } else { 16 };
    for (tag, value) in [(0x100, bits), (0x101, bits), (0x102, bits - 1)] {
        object.put(us(Tag(0x28, tag), value));
    }
    if eight_bit {
        object.put_str(Tag(0x28, 4), VR::CS, "PALETTE");
    }
    object.put(if eight_bit {
        let mut bytes: Vec<u8> = values.iter().map(|v| *v as u8).collect();
        bytes.resize(bytes.len().next_multiple_of(2), 0);
        DataElement::new(Tag(0x7fe0, 0x10), VR::OB, PrimitiveValue::U8(bytes.into()))
    } else {
        DataElement::new(
            Tag(0x7fe0, 0x10),
            VR::OW,
            PrimitiveValue::U16(values.into()),
        )
    });
    object
}
#[test]
fn label_maps_supply_every_listed_plane_for_every_segment_without_overlap() {
    let temp = Temp::new();
    let study = sources(&temp);
    let series = &study.series[0];
    // Planes in frame order z4, z0, z2; z0 pixel 1 is a missing source sample.
    let values = [[9u16, 9, 0, 2, 0, 0], [0, 2, 2, 0, 0, 9], [0; 6]].concat();
    for eight_bit in [false, true] {
        let masks = read_segmentation(
            &write_mask(&temp, &labelmap(values.clone(), eight_bit)),
            series,
            || false,
        )
        .unwrap();
        assert_eq!(
            masks
                .infos()
                .map(|i| (
                    i.number,
                    i.positive_cells,
                    i.valid_source_cells,
                    i.supplied_planes
                ))
                .collect::<Vec<_>>(),
            [(9, 3, 3, 3), (2, 3, 2, 3)]
        );
        assert_eq!(masks.overlap_cells(), 0);
        assert_eq!(masks.value(2, [1, 0, 0]), Some(true));
        assert_eq!(masks.value(9, [0, 0, 2]), Some(true));
        assert_eq!(masks.value(9, [0, 0, 1]), Some(false));
        assert!(masks.plane_is_supplied(2, 1));
    }
    let mut undeclared = values.clone();
    undeclared[4] = 7;
    assert!(matches!(
        read_segmentation(
            &write_mask(&temp, &labelmap(undeclared, false)),
            series,
            || false
        ),
        Err(SegmentationError::Mask(_))
    ));
    for case in 0..4 {
        let mut object = labelmap(values.clone(), false);
        match case {
            // Label-map frames must not name one segment.
            0 => {
                object.put(seq(
                    Tag(0x5200, 0x9230),
                    [(4., 1), (0., 9), (2., 5)]
                        .map(|(z, instance)| frame(9, z, instance))
                        .into(),
                ));
            }
            // The same source plane twice.
            1 => {
                object.put(seq(
                    Tag(0x5200, 0x9230),
                    [(4., 1), (4., 1), (2., 5)]
                        .map(|(z, instance)| {
                            let mut item = frame(9, z, instance);
                            item.remove_element(Tag(0x62, 0xa));
                            item
                        })
                        .into(),
                ));
            }
            2 => {
                object.put_str(Tag(0x62, 1), VR::CS, "BINARY");
            }
            _ => {
                object.put(us(Tag(0x28, 0x100), 32));
            }
        }
        assert!(
            read_segmentation(&write_mask(&temp, &object), series, || false).is_err(),
            "case {case}"
        );
    }
}
