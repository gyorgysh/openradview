// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use dicom_core::{Tag, VR};
use std::io::Cursor;

const FOR_PRESENTATION: &str = "1.2.840.10008.5.1.4.1.1.1.2";
const FOR_PROCESSING: &str = "1.2.840.10008.5.1.4.1.1.1.2.1";

fn write_mammogram(path: &Path, class: &str, instance: i32, labels: [&str; 3]) {
    let mut object =
        dicom_object::from_reader(Cursor::new(support::fixture(0., instance, true, false)))
            .unwrap();
    object.put_str(Tag(8, 0x16), VR::UI, class);
    object.update_meta(|meta| meta.media_storage_sop_class_uid = class.into());
    object.put_str(Tag(8, 0x60), VR::CS, "MG");
    // Projection radiographs carry no patient-space plane.
    for tag in [Tag(0x20, 0x32), Tag(0x20, 0x37), Tag(0x20, 0x52)] {
        object.remove_element(tag);
    }
    let [laterality, view, orientation] = labels;
    object.put_str(Tag(0x20, 0x62), VR::CS, laterality);
    object.put_str(Tag(0x18, 0x5101), VR::CS, view);
    object.put_str(Tag(0x20, 0x20), VR::CS, orientation);
    object.write_to_file(path).unwrap();
}

#[test]
fn mammograms_for_presentation_and_processing_open_as_2d_with_declared_labels() {
    for class in [FOR_PRESENTATION, FOR_PROCESSING] {
        let temp = Temp::new();
        write_mammogram(&temp.0.join("right.dcm"), class, 1, ["R", "MLO", "A\\FR"]);
        let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
        let series = &study.series[0];
        assert_eq!(series.modality, "MG");
        let source = series.source.as_ref().unwrap();
        let frame = source.materialize(|| false).unwrap().remove(0);
        assert!(frame.geometry.is_none());
        assert!(frame.monochrome1);
        let labels = source.projection_labels(0).unwrap();
        assert_eq!(labels.summary().as_deref(), Some("R · MLO"));
        assert_eq!(
            labels.edges().unwrap(),
            ["P", "A", "HL", "FR"].map(String::from)
        );
    }
}

#[test]
fn malformed_projection_labels_are_ignored_instead_of_guessed() {
    let temp = Temp::new();
    write_mammogram(
        &temp.0.join("odd.dcm"),
        FOR_PRESENTATION,
        2,
        ["X", "mlo", "AA\\R"],
    );
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    assert_eq!(
        study.series[0]
            .source
            .as_ref()
            .unwrap()
            .projection_labels(0),
        None
    );
    for bad in [
        &["A"][..],
        &["A", "R", "F"],
        &["Q", "R"],
        &["APRL", "F"],
        &["", "F"],
    ] {
        assert_eq!(crate::inspection::patient_orientation(bad), None, "{bad:?}");
    }
    assert_eq!(
        crate::inspection::patient_orientation(&["L", "F"]),
        Some(["L".into(), "F".into()])
    );
}
