// SPDX-License-Identifier: GPL-3.0-or-later
use super::real_world::phantom;
use super::*;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use std::sync::Arc;

/// A 4x4 unsigned 8-bit image with stored codes 0..15 and no other layers.
fn image(syntax: &str, frames: u32) -> DefaultDicomObject {
    let mut object = phantom::integer(syntax, phantom::item(1., 0., Some((0, 255)), None));
    object.remove_element(Tag(0x0040, 0x9096));
    for tag in [Tag(0x0028, 0x0010), Tag(0x0028, 0x0011)] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(4u16)));
    }
    if frames > 1 {
        object.put_str(Tag(0x0028, 0x0008), VR::IS, frames.to_string());
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8((0..16 * frames).map(|v| v as u8).collect()),
    ));
    object
}
fn bytes(object: &DefaultDicomObject) -> Vec<u8> {
    let mut data = Vec::new();
    object.write_all(&mut data).unwrap();
    data
}
fn both(object: &DefaultDicomObject, frame: usize) -> [Arc<crate::study::Frame>; 2] {
    let eager = crate::load::decode(&bytes(object))
        .unwrap()
        .frames
        .remove(frame);
    let temp = Temp::new();
    let path = temp.0.join("layers.dcm");
    fs::write(&path, bytes(object)).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let lazy = study.series[0].read_frame(frame, || false).unwrap();
    [eager, lazy]
}
fn overlay(object: &mut DefaultDicomObject, group: u16, rows: u16, columns: u16, origin: [i16; 2]) {
    for (element, value) in [(0x0010, rows), (0x0011, columns), (0x0100, 1), (0x0102, 0)] {
        object.put(DataElement::new(
            Tag(group, element),
            VR::US,
            PrimitiveValue::from(value),
        ));
    }
    object.put_str(Tag(group, 0x0040), VR::CS, "G");
    object.put(DataElement::new(
        Tag(group, 0x0050),
        VR::SS,
        PrimitiveValue::I16(origin.to_vec().into()),
    ));
    object.put_str(Tag(group, 0x0022), VR::LO, "Generated overlay description");
    object.put_str(Tag(group, 0x1500), VR::LO, "GENERATED_LABEL");
}
fn marked(rgba: &[u8]) -> Vec<usize> {
    rgba.chunks(4)
        .enumerate()
        .filter(|(_, pixel)| *pixel == [255, 255, 255, 255])
        .map(|(index, _)| index)
        .collect()
}

#[test]
fn overlay_origins_word_packing_and_frame_ranges_match_one_based_definitions() {
    let mut object = image(phantom_syntax(), 3);
    // A 2x2 overlay at origin row 2, column 3: covers rows 2-3, columns 3-4.
    // OW cells start at the low bit: word 0b1001 marks overlay pixels 0 and 3,
    // image (row 2, column 3) and (3, 4), zero-based indices 6 and 11.
    overlay(&mut object, 0x6000, 2, 2, [2, 3]);
    object.put(DataElement::new(
        Tag(0x6000, 0x3000),
        VR::OW,
        PrimitiveValue::U16(vec![0b1001].into()),
    ));
    // A 1x4 overlay on row 4 (indices 12-15) applies only to image frames
    // 2-3 (one-based frame origin 2, two frames).
    overlay(&mut object, 0x6002, 1, 4, [4, 1]);
    object.put_str(Tag(0x6002, 0x0015), VR::IS, "2");
    object.put(DataElement::new(
        Tag(0x6002, 0x0051),
        VR::US,
        PrimitiveValue::from(2u16),
    ));
    object.put(DataElement::new(
        Tag(0x6002, 0x3000),
        VR::OB,
        // Planes are contiguous four-bit runs: plane 0 bit 0, plane 1 bit 3.
        PrimitiveValue::U8(vec![0b1000_0001].into()),
    ));
    for (frame, expected) in [(0, vec![6, 11]), (1, vec![6, 11, 12]), (2, vec![6, 11, 15])] {
        for decoded in both(&object, frame) {
            let layers = decoded.presentation.as_ref().unwrap();
            assert_eq!((layers.overlay_count(), layers.unsupported), (2, 0));
            let rgba = layers.rgba(decoded.frame_index, 4, 4, true, false).unwrap();
            assert_eq!(marked(&rgba), expected, "frame {frame}");
            assert!(
                layers
                    .rgba(decoded.frame_index, 4, 4, false, false)
                    .unwrap()
                    .chunks(4)
                    .all(|p| p[3] == 0)
            );
            // Source samples are untouched.
            assert_eq!(decoded.pixels[6], 16. * frame as f32 + 6.);
        }
    }
    let debug = format!("{:?}", both(&object, 0)[0].presentation);
    assert!(!debug.contains("GENERATED_LABEL") && !debug.contains("Generated overlay"));
}
fn phantom_syntax() -> &'static str {
    "1.2.840.10008.1.2.1"
}

#[test]
fn shutters_intersect_display_only_and_keep_statistics_and_validity() {
    let mut object = image(phantom_syntax(), 1);
    object.put_str(Tag(0x0018, 0x1600), VR::CS, "RECTANGULAR\\CIRCULAR");
    for (element, value) in [(0x1602, "1"), (0x1604, "3"), (0x1606, "1"), (0x1608, "4")] {
        object.put_str(Tag(0x0018, element), VR::IS, value);
    }
    object.put_str(Tag(0x0018, 0x1610), VR::IS, "2\\2");
    object.put_str(Tag(0x0018, 0x1612), VR::IS, "1");
    object.put(DataElement::new(
        Tag(0x0018, 0x1622),
        VR::US,
        PrimitiveValue::from(65535u16),
    ));
    for decoded in both(&object, 0) {
        let layers = decoded.presentation.as_ref().unwrap();
        assert!(layers.has_shutter());
        let rgba = layers.rgba(0, 4, 4, false, true).unwrap();
        // Exposed: columns 1-3 and within radius 1 of row 2, column 2.
        let exposed: Vec<usize> = rgba
            .chunks(4)
            .enumerate()
            .filter(|(_, p)| p[3] == 0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(exposed, vec![1, 4, 5, 6, 9]);
        assert!(
            rgba.chunks(4)
                .filter(|p| p[3] == 255)
                .all(|p| p[..3] == [255; 3])
        );
        assert!(decoded.validity.is_none());
        assert_eq!(
            crate::inspection::sample(&decoded, [3., 3.]),
            Some(crate::inspection::Sample::Scalar(15.))
        );
    }
    let mut polygon = image(phantom_syntax(), 1);
    polygon.put_str(Tag(0x0018, 0x1600), VR::CS, "POLYGONAL");
    polygon.put_str(Tag(0x0018, 0x1620), VR::IS, "0\\0\\0\\5\\5\\0");
    let rgba = both(&polygon, 0)[1]
        .presentation
        .as_ref()
        .unwrap()
        .rgba(0, 4, 4, false, true)
        .unwrap();
    // Pixel centres strictly below the diagonal row + column = 5 are exposed.
    for (index, pixel) in rgba.chunks(4).enumerate() {
        let (row, column) = (index / 4 + 1, index % 4 + 1);
        assert_eq!(pixel[3] == 0, row + column < 5, "{row},{column}");
        assert_eq!(pixel[..3], [0; 3]);
    }
}

#[test]
fn unsupported_layers_are_counted_without_refusing_the_image() {
    for variant in 0..6 {
        let mut object = image(phantom_syntax(), 1);
        match variant {
            0 => {
                overlay(&mut object, 0x6000, 4, 4, [1, 1]);
                object.put(DataElement::new(
                    Tag(0x6000, 0x0100),
                    VR::US,
                    PrimitiveValue::from(16u16),
                ));
                object.put(DataElement::new(
                    Tag(0x6000, 0x3000),
                    VR::OB,
                    PrimitiveValue::U8(vec![0; 2].into()),
                ));
            }
            1 => {
                // 8x4 needs four bytes; even-length encoding still leaves two.
                overlay(&mut object, 0x6000, 8, 4, [1, 1]);
                object.put(DataElement::new(
                    Tag(0x6000, 0x3000),
                    VR::OB,
                    PrimitiveValue::U8(vec![0; 2].into()),
                ));
            }
            2 => {
                object.put_str(Tag(0x0018, 0x1600), VR::CS, "BITMAP");
            }
            3 => {
                object.put_str(Tag(0x0018, 0x1600), VR::CS, "CIRCULAR");
                object.put_str(Tag(0x0018, 0x1610), VR::IS, "2\\2");
                object.put_str(Tag(0x0018, 0x1612), VR::IS, "1");
                object.put(DataElement::new(
                    Tag(0x0018, 0x1624),
                    VR::US,
                    PrimitiveValue::U16(vec![0, 0, 0].into()),
                ));
            }
            4 => {
                object.put_str(Tag(0x0018, 0x1600), VR::CS, "RECTANGULAR");
                for (element, value) in [(0x1602, "3"), (0x1604, "1"), (0x1606, "1"), (0x1608, "4")]
                {
                    object.put_str(Tag(0x0018, element), VR::IS, value);
                }
            }
            _ => {
                overlay(&mut object, 0x6000, 4, 4, [1, 1]);
                object.put_str(Tag(0x6000, 0x0015), VR::IS, "2");
                object.put(DataElement::new(
                    Tag(0x6000, 0x3000),
                    VR::OB,
                    PrimitiveValue::U8(vec![0; 4].into()),
                ));
            }
        }
        for decoded in both(&object, 0) {
            let layers = decoded.presentation.as_ref().unwrap();
            assert_eq!(layers.unsupported, 1, "variant{variant}");
            assert_eq!(layers.overlay_count(), 0);
            assert!(!layers.has_shutter());
            assert_eq!(decoded.pixels[15], 15.);
        }
    }
    for decoded in both(&image(phantom_syntax(), 1), 0) {
        assert!(decoded.presentation.is_none());
    }
}

#[test]
fn overlay_padding_is_not_retained_and_many_vertex_shutters_stay_row_bounded() {
    let mut object = image(phantom_syntax(), 1);
    // A 3x3 overlay needs two bytes; the supplied value carries 64.
    overlay(&mut object, 0x6000, 3, 3, [1, 1]);
    object.put(DataElement::new(
        Tag(0x6000, 0x3000),
        VR::OB,
        PrimitiveValue::U8(vec![0xff; 64].into()),
    ));
    // A 512-vertex polygon tracing the square rows 1.5-3.5, columns 1.5-3.5.
    let mut vertices = Vec::new();
    for corner in [[1.5, 1.5], [1.5, 3.5], [3.5, 3.5], [3.5, 1.5]] {
        vertices.push(corner);
    }
    let mut points = Vec::new();
    for edge in 0..4 {
        let (a, b): ([f64; 2], [f64; 2]) = (vertices[edge], vertices[(edge + 1) % 4]);
        for step in 0..128 {
            let t = f64::from(step) / 128.;
            points.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    // IS values are integers; scale both axes by 10 on a 40x40 image.
    let text: Vec<String> = points
        .iter()
        .flat_map(|p| {
            [
                ((p[0] * 10.) as i64).to_string(),
                ((p[1] * 10.) as i64).to_string(),
            ]
        })
        .collect();
    object.put_str(Tag(0x0018, 0x1600), VR::CS, "POLYGONAL");
    object.put_str(Tag(0x0018, 0x1620), VR::IS, text.join("\\"));
    for decoded in both(&object, 0) {
        let layers = decoded.presentation.as_ref().unwrap();
        assert_eq!(layers.retained_bytes(), 2);
        assert!(layers.has_shutter());
        let rgba = layers.rgba(0, 40, 40, false, true).unwrap();
        for (index, pixel) in rgba.chunks(4).enumerate() {
            let (row, column) = (index / 40 + 1, index % 40 + 1);
            // Integer vertices put some centres exactly on the boundary,
            // where inclusion is a convention; check every other pixel.
            if [row, column].iter().any(|v| *v == 15 || *v == 35) {
                continue;
            }
            let inside = (16..=34).contains(&row) && (16..=34).contains(&column);
            assert_eq!(pixel[3] == 0, inside, "{row},{column}");
        }
    }
}
