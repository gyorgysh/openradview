// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use dicom_core::{Tag, VR};
use std::io::Cursor;

#[test]
fn ultrasound_regions_reach_frames_through_indexed_and_eager_decoding() {
    let temp = Temp::new();
    let mut object =
        dicom_object::from_reader(Cursor::new(support::fixture(0., 1, false, false))).unwrap();
    object.put_str(Tag(8, 0x16), VR::UI, "1.2.840.10008.5.1.4.1.1.6.1");
    object.update_meta(|meta| {
        meta.media_storage_sop_class_uid = "1.2.840.10008.5.1.4.1.1.6.1".into()
    });
    object.put_str(Tag(8, 0x60), VR::CS, "US");
    for tag in [Tag(0x20, 0x32), Tag(0x20, 0x37), Tag(0x20, 0x52)] {
        object.remove_element(tag);
    }
    let regions = crate::calibration::tests::object(vec![crate::calibration::tests::region(
        [0, 0, 1, 1],
        [0.05, 0.1],
        3,
    )]);
    for element in regions {
        object.put(element);
    }
    let path = temp.0.join("us.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    let indexed = study.series[0]
        .source
        .as_ref()
        .unwrap()
        .materialize(|| false)
        .unwrap()
        .remove(0);
    let eager = crate::load::decode(&fs::read(&path).unwrap())
        .unwrap()
        .frames
        .remove(0);
    for frame in [indexed, eager] {
        assert!(frame.geometry.is_none());
        let calibration = frame.calibration.as_ref().unwrap();
        assert_eq!(calibration.regions()[0].spacing_mm, [0.5, 1.]);
    }
}
