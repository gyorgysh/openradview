// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::import_report::SkipReason;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::meta::FileMetaTableBuilder;
use std::io::Cursor;
const CLASS: &str = "1.2.840.10008.5.1.4.1.1.66.4";
fn binary() -> Vec<u8> {
    let mut object =
        dicom_object::from_reader(Cursor::new(support::fixture(0., 1, false, false))).unwrap();
    object.meta_mut().media_storage_sop_class_uid = CLASS.into();
    object.meta_mut().update_information_group_length();
    object.put_str(Tag(0x0008, 0x0016), VR::UI, CLASS);
    object.put_str(Tag(0x0062, 0x0001), VR::CS, "BINARY");
    object.put(DataElement::new(
        Tag(0x0028, 0x0011),
        VR::US,
        PrimitiveValue::from(3u16),
    ));
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "2");
    for tag in [0x0100, 0x0101] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(1u16),
        ));
    }
    object.put(DataElement::new(
        Tag(0x0028, 0x0102),
        VR::US,
        PrimitiveValue::from(0u16),
    ));
    object.put(DataElement::new(
        Tag(0x0028, 0x0103),
        VR::US,
        PrimitiveValue::from(0u16),
    ));
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8(vec![0xab, 0x06].into()),
    ));
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    bytes
}
#[test]
fn prefix_stops_at_native_payload_without_allocating_pixels_or_changing_its_reader_offset() {
    let bytes = binary();
    let mut input = Cursor::new(&bytes);
    let mut reason = SkipReason::UnrecognizedInput;
    let prefix =
        header::read_prefix(&mut input, &|| false, &mut reason, |class| class == CLASS).unwrap();
    assert_eq!(prefix.syntax, "1.2.840.10008.1.2.1");
    assert_eq!(prefix.deflated_at, None);
    assert_eq!(prefix.pixel_length, Some(2));
    assert_eq!(prefix.pixel_offset, input.position());
    assert_eq!(prefix.pixel_offset as usize + 2, bytes.len());
    assert!(prefix.object.element(Tag(0x7fe0, 0x0010)).is_err());
    assert_eq!(
        crate::load::integer(&prefix.object, "NumberOfFrames"),
        Some(2)
    );
    let mut pixels = [0; 2];
    input.read_exact(&mut pixels).unwrap();
    assert_eq!(pixels, [0xab, 0x06]);
}
#[test]
fn class_admission_runs_before_dataset_read_and_cancellation_retains_no_prefix() {
    let bytes = binary();
    let mut input = Cursor::new(&bytes);
    let mut reason = SkipReason::UnrecognizedInput;
    assert!(matches!(
        header::read_prefix(&mut input, &|| false, &mut reason, |_| false),
        Err(LoadError::Unsupported)
    ));
    assert_eq!(reason, SkipReason::UnsupportedObject);
    let stopped = input.position() as usize;
    assert!(bytes[stopped..].starts_with(&[8, 0, 0x16, 0]));
    let mut input = Cursor::new(&bytes);
    assert!(header::read_prefix(&mut input, &|| true, &mut reason, |_| true).is_err());
}
#[test]
fn declared_pixel_bytes_are_not_allocated_but_oversized_metadata_and_deep_nesting_are_refused() {
    let mut bytes = binary();
    let payload_header = bytes.len() - 14;
    assert_eq!(
        &bytes[payload_header..payload_header + 4],
        &[0xe0, 0x7f, 0x10, 0]
    );
    bytes[payload_header + 8..payload_header + 12].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
    bytes.truncate(payload_header + 12);
    let prefix = header::read_prefix(
        &mut Cursor::new(&bytes),
        &|| false,
        &mut SkipReason::UnrecognizedInput,
        |_| true,
    )
    .unwrap();
    assert_eq!(prefix.pixel_length, Some(u32::MAX as u64 - 1));
    let mut meta = vec![0; 128];
    meta.extend(b"DICM");
    FileMetaTableBuilder::new()
        .media_storage_sop_class_uid(CLASS)
        .media_storage_sop_instance_uid("2.25.123456")
        .transfer_syntax("1.2.840.10008.1.2.1")
        .build()
        .unwrap()
        .write(&mut meta)
        .unwrap();
    let mut huge = meta.clone();
    huge.extend([0x77, 0x77, 0x10, 0, b'O', b'B', 0, 0]);
    huge.extend((9u32 * 1024 * 1024).to_le_bytes());
    assert!(matches!(
        header::read_prefix(
            &mut Cursor::new(huge),
            &|| false,
            &mut SkipReason::UnrecognizedInput,
            |_| true
        ),
        Err(LoadError::Limit)
    ));
    for _ in 0..33 {
        meta.extend([8, 0, 0x11, 0x11, b'S', b'Q', 0, 0]);
        meta.extend(u32::MAX.to_le_bytes());
        meta.extend([0xfe, 0xff, 0, 0xe0]);
        meta.extend(u32::MAX.to_le_bytes());
    }
    assert!(matches!(
        header::read_prefix(
            &mut Cursor::new(meta),
            &|| false,
            &mut SkipReason::UnrecognizedInput,
            |_| true
        ),
        Err(LoadError::Limit)
    ));
}
#[test]
fn supplied_mask_classes_never_become_ordinary_images_even_with_disagreeing_file_meta() {
    for class in [
        CLASS,
        "1.2.840.10008.5.1.4.1.1.66.5",
        "1.2.840.10008.5.1.4.1.1.66.7",
        "1.2.840.10008.5.1.4.1.1.66.8",
    ] {
        for disagrees in [false, true] {
            let mut object =
                dicom_object::from_reader(Cursor::new(support::fixture(0., 1, false, false)))
                    .unwrap();
            if !disagrees {
                object.meta_mut().media_storage_sop_class_uid = class.into();
                object.meta_mut().update_information_group_length();
            }
            object.put_str(Tag(0x0008, 0x0016), VR::UI, class);
            let mut bytes = Vec::new();
            object.write_all(&mut bytes).unwrap();
            let failure = header::read_header_detailed(&mut Cursor::new(&bytes), || false)
                .err()
                .unwrap();
            assert_eq!(failure.reason, SkipReason::UnsupportedObject);
            assert!(matches!(
                crate::load::decode(&bytes),
                Err(LoadError::Unsupported)
            ));
        }
    }
}
