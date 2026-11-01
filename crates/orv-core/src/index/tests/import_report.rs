// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::import_report::SkipReason;
use dicom_core::{Tag, VR};
use std::io::Cursor;

fn mixed_inputs() -> Vec<(&'static str, Vec<u8>)> {
    let valid = support::fixture(0., 1, false, false);
    let mut unsupported = dicom_object::from_reader(Cursor::new(&valid)).unwrap();
    unsupported.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.100".into();
    unsupported.meta_mut().update_information_group_length();
    let mut encoding = Vec::new();
    unsupported.write_all(&mut encoding).unwrap();

    let mut report = dicom_object::from_reader(Cursor::new(&valid)).unwrap();
    report.meta_mut().media_storage_sop_class_uid = "1.2.840.10008.5.1.4.1.1.88.11".into();
    report.meta_mut().update_information_group_length();
    report.put_str(Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.88.11");
    report.remove_element(Tag(0x7fe0, 0x0010));
    let mut document = Vec::new();
    report.write_all(&mut document).unwrap();

    let mut representation = dicom_object::from_reader(Cursor::new(&valid)).unwrap();
    representation.put_str(Tag(0x0028, 0x0004), VR::CS, "HSV");
    let mut other = Vec::new();
    representation.write_all(&mut other).unwrap();
    vec![
        ("01-valid.dcm", valid.clone()),
        ("02-encoding.dcm", encoding),
        ("03-document.dcm", document),
        ("04-short.dcm", valid[..150].to_vec()),
        ("05-unrecognized.txt", b"Generated non-image input".to_vec()),
        ("06-orphan.hdr", vec![0; 348]),
        ("07-representation.dcm", other),
    ]
}

#[test]
fn mixed_file_and_zip_imports_explain_skips_without_retaining_source_names() {
    for archived in [false, true] {
        let temp = Temp::new();
        let path = if archived {
            let path = temp.0.join("generated.zip");
            let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
            for (name, bytes) in mixed_inputs() {
                archive
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                archive.write_all(&bytes).unwrap();
            }
            archive.finish().unwrap();
            path
        } else {
            for (name, bytes) in mixed_inputs() {
                fs::write(temp.0.join(name), bytes).unwrap();
            }
            temp.0.clone()
        };
        let study = index_paths_detailed(&[path], || false, |_| {}).unwrap();
        assert_eq!(study.frame_count(), 1);
        assert_eq!(study.import_report.indexed_files(), 1);
        assert_eq!(study.skipped_files, 6);
        assert_eq!(study.import_report.skipped_files(), 6);
        for reason in SkipReason::ALL {
            let expected = usize::from(!matches!(
                reason,
                SkipReason::MissingReference | SkipReason::ConflictingReference
            ));
            assert_eq!(study.import_report.count(reason), expected, "{reason:?}");
        }
        let reasons: Vec<_> = study
            .import_report
            .issues()
            .iter()
            .map(|issue| (issue.item, issue.reason))
            .collect();
        assert_eq!(
            reasons,
            vec![
                (2, SkipReason::UnsupportedEncoding),
                (3, SkipReason::UnsupportedObject),
                (4, SkipReason::MalformedMetadata),
                (5, SkipReason::UnrecognizedInput),
                (6, SkipReason::MissingCompanion),
                (7, SkipReason::UnsupportedOrInvalid),
            ]
        );
        let debug = format!("{:?}", study.import_report);
        for text in [
            "generated.zip",
            "Synthetic",
            "orphan",
            "document.dcm",
            "2.25.",
        ] {
            assert!(!debug.contains(text));
        }
        assert_eq!(
            &study.series[0].read_frame(0, || false).unwrap().pixels[..],
            &[-100., 0., 100., -102.]
        );
    }
}

#[test]
fn empty_import_retains_exact_counts_with_bounded_issue_examples() {
    let temp = Temp::new();
    for n in 0..90 {
        fs::write(temp.0.join(format!("item-{n:03}.txt")), b"synthetic").unwrap();
    }
    let failure =
        index_paths_detailed(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap_err();
    assert_eq!(failure.error, LoadError::Empty);
    assert_eq!(failure.report.indexed_files(), 0);
    assert_eq!(failure.report.count(SkipReason::UnrecognizedInput), 90);
    assert_eq!(failure.report.issues().len(), 64);
    assert_eq!(failure.report.omitted_issues(), 26);
    assert!(failure.report.issues().iter().all(|issue| issue.item <= 64));
    assert!(!failure.to_string().contains("item-"));
    assert!(failure.to_string().contains("90 unrecognized"));
    assert_eq!(
        index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap_err(),
        LoadError::Empty
    );
}

#[test]
fn cancellation_and_limits_remain_failures_instead_of_skipped_inputs() {
    let temp = Temp::new();
    fs::write(temp.0.join("01-other.txt"), b"generated").unwrap();
    fs::write(
        temp.0.join("02-image.dcm"),
        support::fixture(0., 1, false, false),
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    let failure = index_paths_detailed(
        std::slice::from_ref(&temp.0),
        || cancel.load(Ordering::Relaxed),
        |_| cancel.store(true, Ordering::Relaxed),
    )
    .unwrap_err();
    assert_eq!(failure.error, LoadError::Cancelled);
    assert_eq!(failure.report.skipped_files(), 1);
    let path = temp.0.join("03-large.dcm");
    fs::write(&path, support::fixture(0., 1, false, true)).unwrap();
    let failure = index_paths_detailed(&[path], || false, |_| {}).unwrap_err();
    assert_eq!(failure.error, LoadError::Limit);
    assert_eq!(failure.report.skipped_files(), 0);
}
