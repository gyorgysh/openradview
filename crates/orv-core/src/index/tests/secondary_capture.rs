// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use std::io::Cursor;

/// Multi-frame secondary capture classes, as produced by reporting or model tools.
fn capture(class: &str, bits: u16, rgb: bool) -> dicom_object::DefaultDicomObject {
    let mut object =
        dicom_object::from_reader(Cursor::new(support::fixture(0., 1, false, false))).unwrap();
    object.put_str(Tag(8, 0x16), VR::UI, class);
    object.update_meta(|meta| meta.media_storage_sop_class_uid = class.into());
    object.put_str(Tag(8, 0x60), VR::CS, "OT");
    for tag in [
        Tag(0x20, 0x32),
        Tag(0x20, 0x37),
        Tag(0x20, 0x52),
        Tag(0x28, 0x30),
        Tag(0x28, 0x1050),
        Tag(0x28, 0x1051),
        Tag(0x28, 0x1052),
        Tag(0x28, 0x1053),
    ] {
        object.remove_element(tag);
    }
    object.put_str(Tag(0x28, 8), VR::IS, "2");
    let us = |tag, v: u16| DataElement::new(tag, VR::US, PrimitiveValue::from(v));
    for (tag, value) in [(0x100, bits), (0x101, bits), (0x102, bits - 1), (0x103, 0)] {
        object.put(us(Tag(0x28, tag), value));
    }
    let samples = if rgb { 3 } else { 1 };
    object.put(us(Tag(0x28, 2), samples));
    if rgb {
        object.put_str(Tag(0x28, 4), VR::CS, "RGB");
        object.put(us(Tag(0x28, 6), 0));
    }
    let count = 2 * 2 * 2 * usize::from(samples);
    object.put(if bits == 8 {
        DataElement::new(
            Tag(0x7fe0, 0x10),
            VR::OB,
            PrimitiveValue::U8((0..count as u8).map(|v| v * 10).collect()),
        )
    } else {
        DataElement::new(
            Tag(0x7fe0, 0x10),
            VR::OW,
            PrimitiveValue::U16((0..count as u16).map(|v| v * 1000).collect()),
        )
    });
    object
}

#[test]
fn multiframe_grayscale_and_true_colour_secondary_captures_open_frame_by_frame() {
    for (class, bits, rgb) in [
        ("1.2.840.10008.5.1.4.1.1.7", 8, false),
        ("1.2.840.10008.5.1.4.1.1.7.2", 8, false),
        ("1.2.840.10008.5.1.4.1.1.7.3", 16, false),
        ("1.2.840.10008.5.1.4.1.1.7.4", 8, true),
    ] {
        let temp = Temp::new();
        let path = temp.0.join("capture.dcm");
        capture(class, bits, rgb).write_to_file(&path).unwrap();
        let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
        let series = &study.series[0];
        let frames = series
            .source
            .as_ref()
            .unwrap()
            .materialize(|| false)
            .unwrap();
        assert_eq!(frames.len(), 2, "{class}");
        for frame in &frames {
            assert!(frame.geometry.is_none(), "{class}");
            assert_eq!(frame.rgba.is_some(), rgb, "{class}");
        }
        if !rgb {
            let scale = if bits == 8 { 10. } else { 1000. };
            assert_eq!(frames[1].pixels[0], 4. * scale, "{class}");
        }
    }
}
