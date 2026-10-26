// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::test_support as support;
use std::io::Write;

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("orv-synthetic-test-{}-{ns}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn signed_rescale_once_and_photometric_remains_a_display_flag() {
    for invert in [false, true] {
        let series = decode(&support::fixture(0.0, 1, invert, false)).unwrap();
        let frame = &series.frames[0];
        assert_eq!(&*frame.pixels, &[-100.0, 0.0, 100.0, -102.0]);
        assert_eq!(frame.monochrome1, invert);
        assert_eq!(
            frame.default_window,
            WindowLevel {
                width: 200.0,
                center: 0.0
            }
        );
        assert_eq!(frame.aspect_ratio(), 0.5);
        assert_eq!(series.label(), "CT · Series 7");
        assert!(!format!("{series:?}").contains("Synthetic^Only"));
    }
}

#[test]
fn quadruped_images_remain_viewable_without_assumed_biped_geometry() {
    let mut obj =
        dicom_object::from_reader(Cursor::new(support::fixture(0.0, 1, false, false))).unwrap();
    obj.put(dicom_core::DataElement::new(
        dicom_core::Tag(0x0010, 0x2210),
        dicom_core::VR::CS,
        "QUADRUPED",
    ));
    let mut bytes = Vec::new();
    obj.write_all(&mut bytes).unwrap();
    let series = decode(&bytes).unwrap();
    assert!(series.frames[0].geometry.is_none());
    assert_eq!(series.frames[0].pixels.len(), 4);
}

#[test]
fn folder_groups_and_sorts_by_spatial_location_not_filename_or_instance() {
    let temp = Temp::new();
    fs::write(temp.0.join("a.dcm"), support::fixture(4.0, 1, false, false)).unwrap();
    fs::write(temp.0.join("z.dcm"), support::fixture(2.0, 9, false, false)).unwrap();
    fs::write(temp.0.join("readme.txt"), b"not an image").unwrap();
    let mut progress = 0;
    let study = load_paths(
        std::slice::from_ref(&temp.0),
        &AtomicBool::new(false),
        |n| progress = n,
    )
    .unwrap();
    assert_eq!(study.series.len(), 1);
    assert_eq!(study.frame_count(), 2);
    assert_eq!(study.skipped_files, 1);
    assert_eq!(study.series[0].frames[0].instance, 9);
    assert_eq!(progress, 3);
}

#[test]
fn rejects_oversized_geometry_and_obeys_cancellation() {
    assert_eq!(
        decode(&support::fixture(0.0, 1, false, true)).unwrap_err(),
        LoadError::Limit
    );
    let error = load_paths(
        &[PathBuf::from("synthetic/nonexistent")],
        &AtomicBool::new(true),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error, LoadError::Cancelled);
    assert!(!error.to_string().contains("nonexistent"));
}

#[test]
fn zip_loads_in_memory_and_rejects_traversal() {
    let temp = Temp::new();
    for (name, valid) in [("nested/synthetic.dcm", true), ("../escape.dcm", false)] {
        let path = temp.0.join(if valid { "valid.zip" } else { "invalid.zip" });
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&support::fixture(0.0, 1, false, false))
            .unwrap();
        zip.finish().unwrap();
        let result = load_paths(&[path], &AtomicBool::new(false), |_| {});
        if valid {
            assert_eq!(result.unwrap().frame_count(), 1);
        } else {
            assert_eq!(result.unwrap_err(), LoadError::Unsupported);
        }
    }
    assert!(!temp.0.join("nested").exists());
    assert!(!temp.0.join("escape.dcm").exists());
}
