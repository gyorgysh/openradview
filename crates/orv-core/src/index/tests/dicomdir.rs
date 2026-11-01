// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{import_report::SkipReason, index::dicomdir};
use dicom_core::{Tag, VR};
use std::io::Cursor;
#[path = "../../../tests/support/media.rs"]
mod media;

fn image(instance: i32) -> Vec<u8> {
    let mut object = dicom_object::from_reader(Cursor::new(support::fixture(
        f64::from(instance) * 2.,
        instance,
        false,
        false,
    )))
    .unwrap();
    object.put_str(Tag(0x20, 0xd), VR::UI, media::STUDY);
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    bytes
}
fn write_media(root: &Path, bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(root.join("Images")).unwrap();
    fs::write(root.join("Images/Img1"), image(1)).unwrap();
    fs::write(root.join("Images/Img2"), image(2)).unwrap();
    let path = root.join("DICOMDIR");
    fs::write(&path, bytes).unwrap();
    path
}
fn parse(bytes: &[u8]) -> Result<dicomdir::Directory, LoadError> {
    dicomdir::read(&mut Cursor::new(bytes), bytes.len() as u64, &|| false)
}
#[test]
fn directory_offsets_not_physical_record_order_define_the_same_local_and_zip_images() {
    for undefined in [false, true] {
        let (bytes, _) = media::directory(&media::records(), &[4, 1, 3, 0, 2], undefined);
        assert_eq!(parse(&bytes).unwrap().references.len(), 2);
        for compression in [
            None,
            Some(zip::CompressionMethod::Stored),
            Some(zip::CompressionMethod::Deflated),
        ] {
            let temp = Temp::new();
            let directory = write_media(&temp.0, &bytes);
            let path = if let Some(compression) = compression {
                let path = temp.0.join("generated.zip");
                let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
                // A nested file-set and reversed members cannot affect reference identity.
                for (name, bytes) in [
                    ("MEDIA/Images/Img2", image(2)),
                    ("MEDIA/DICOMDIR", bytes.clone()),
                    ("MEDIA/Images/Img1", image(1)),
                    ("unlisted.dcm", image(3)),
                ] {
                    archive
                        .start_file(
                            name,
                            zip::write::SimpleFileOptions::default()
                                .compression_method(compression),
                        )
                        .unwrap();
                    archive.write_all(&bytes).unwrap();
                }
                archive.finish().unwrap();
                path
            } else {
                directory
            };
            let study = index_paths_detailed(&[path], || false, |_| {}).unwrap();
            assert_eq!(study.frame_count(), 2);
            assert_eq!(study.import_report.directories(), 1);
            assert_eq!(study.import_report.indexed_files(), 2);
            assert_eq!(study.skipped_files, 0);
            assert!(study.series[0].frames.is_empty());
            for (i, instance) in [(0, 1), (1, 2)] {
                let frame = study.series[0].read_frame(i, || false).unwrap();
                assert_eq!(frame.instance, instance);
                assert_eq!(&frame.pixels[..], &[-100., 0., 100., -102.]);
            }
            let public = format!("{study:?} {:?}", study.import_report);
            for private in ["DirectoryOnly", media::STUDY, "Images", "Img1", "MEDIA/"] {
                assert!(!public.contains(private));
            }
        }
    }
}

#[test]
fn directory_missing_conflicting_and_unsupported_records_have_distinct_bounded_reasons() {
    let temp = Temp::new();
    let mut records = media::records();
    records[4].file = Some("IMAGES\\ABSENT".into());
    records.push(records[3].clone());
    records[4].next = Some(5);
    records[5].file = Some("IMAGES\\WRONG".into());
    records[5].instance = "2.25.123456".into();
    records[5].next = Some(6);
    records.push(records[4].clone());
    records[6].kind = "SR DOCUMENT";
    records[6].file = Some("IMAGES\\REPORT".into());
    records[6].instance = "2.25.999999".into();
    records[6].next = None;
    let (bytes, _) = media::directory(&records, &(0..7).collect::<Vec<_>>(), true);
    let path = write_media(&temp.0, &bytes);
    fs::write(temp.0.join("Images/WRONG"), image(2)).unwrap();
    fs::write(
        temp.0.join("Images/REPORT"),
        b"generated unsupported object",
    )
    .unwrap();
    let study = index_paths_detailed(&[path], || false, |_| {}).unwrap();
    assert_eq!(study.frame_count(), 1);
    assert_eq!(study.skipped_files, 3);
    for reason in [
        SkipReason::MissingReference,
        SkipReason::ConflictingReference,
        SkipReason::UnsupportedObject,
    ] {
        assert_eq!(study.import_report.count(reason), 1);
    }
    let error = format!("{:?}", study.import_report);
    for private in ["ABSENT", "WRONG", "REPORT", "2.25."] {
        assert!(!error.contains(private));
    }
}

#[test]
fn directory_rejects_cycles_orphans_wrong_hierarchy_and_duplicate_file_or_instance_references() {
    let base = media::records();
    for case in 0..7 {
        let mut records = base.clone();
        match case {
            0 => records[4].next = Some(3),
            1 => records[3].lower = Some(0),
            2 => records[3].next = None,
            3 => records[1].kind = "SERIES",
            4 => records[4].file = records[3].file.clone(),
            5 => records[4].instance = records[3].instance.clone(),
            6 => records[0].active = 0,
            _ => unreachable!(),
        }
        let (bytes, _) = media::directory(&records, &[0, 1, 2, 3, 4], true);
        assert!(
            matches!(parse(&bytes), Err(LoadError::Directory)),
            "case {case}"
        );
    }
    let (mut bytes, offsets) = media::directory(&base, &[0, 1, 2, 3, 4], false);
    // A pointer into an Item's payload is not a record offset.
    let at = offsets[0] as usize + 8 + 12 + 10 + 8;
    bytes[at..at + 4].copy_from_slice(&(offsets[1] + 8).to_le_bytes());
    assert!(matches!(parse(&bytes), Err(LoadError::Directory)));
}

#[test]
fn directory_file_ids_cannot_express_ambient_paths_or_ambiguous_case_names() {
    for file in [
        "..\\SECRET",
        "/SECRET",
        "C:\\SECRET",
        "\\\\HOST\\FILE",
        "IMAGES/IMG1",
        "IMAGES\\..\\IMG1",
        "IMAGES\\",
        "TOO_LONG_1",
        "IMAGES\\IMG.1",
        "A\\B\\C\\D\\E\\F\\G\\H\\I",
    ] {
        let mut records = media::records();
        records[3].file = Some(file.into());
        let (bytes, _) = media::directory(&records, &[0, 1, 2, 3, 4], true);
        assert!(matches!(parse(&bytes), Err(LoadError::Directory)), "{file}");
    }
    let temp = Temp::new();
    let (bytes, _) = media::directory(&media::records(), &[0, 1, 2, 3, 4], true);
    let path = temp.0.join("ambiguous.zip");
    let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
    for (name, content) in [
        ("DICOMDIR", bytes),
        ("IMAGES/IMG1", image(1)),
        ("images/img1", image(1)),
    ] {
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&content).unwrap();
    }
    archive.finish().unwrap();
    assert_eq!(
        index_paths(&[path], || false, |_| {}).unwrap_err(),
        LoadError::Directory
    );
}

#[test]
fn directory_empty_reserved_active_flags_inactive_records_and_cancellation_are_explicit() {
    let (empty, _) = media::directory(&[], &[], true);
    assert!(parse(&empty).unwrap().references.is_empty());
    let temp = Temp::new();
    let path = temp.0.join("DICOMDIR");
    fs::write(&path, &empty).unwrap();
    let failure = index_paths_detailed(&[path], || false, |_| {}).unwrap_err();
    assert_eq!(failure.error, LoadError::Empty);
    assert_eq!(failure.report.directories(), 1);
    assert_eq!(failure.report.skipped_files(), 0);
    let mut records = media::records();
    records[3].active = 7; // Reserved nonzero in-use values are interpreted as active.
    records[4].active = 0;
    let (bytes, _) = media::directory(&records, &[0, 1, 2, 3, 4], false);
    assert_eq!(parse(&bytes).unwrap().references.len(), 1);
    assert!(matches!(
        dicomdir::read(&mut Cursor::new(&bytes), bytes.len() as u64, &|| true),
        Err(LoadError::Cancelled)
    ));
    assert!(matches!(
        dicomdir::read(&mut Cursor::new([]), 17 * 1024 * 1024, &|| false),
        Err(LoadError::Limit)
    ));
    for n in [0, 131, bytes.len() - 1] {
        assert!(parse(&bytes[..n]).is_err());
    }
}

#[test]
fn folder_scan_is_an_explicit_fallback_for_a_broken_index_and_ignores_its_text() {
    let temp = Temp::new();
    let path = write_media(&temp.0, b"Generated unusable DICOMDIR");
    assert_eq!(
        index_paths(&[path], || false, |_| {}).unwrap_err(),
        LoadError::Directory
    );
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    assert_eq!(study.frame_count(), 2);
    assert_eq!(study.skipped_files, 0);
    assert_eq!(study.import_report.directories(), 0);

    let archived = Temp::new();
    let path = archived.0.join("generated.zip");
    let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
    for (name, bytes) in [
        ("DICOMDIR", b"Generated unusable index".to_vec()),
        ("Images/Img1", image(1)),
    ] {
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes).unwrap();
    }
    archive.finish().unwrap();
    assert_eq!(
        index_paths(&[path], || false, |_| {}).unwrap_err(),
        LoadError::Directory
    );
    let study = index_paths(std::slice::from_ref(&archived.0), || false, |_| {}).unwrap();
    assert_eq!(study.frame_count(), 1);
    assert_eq!(study.skipped_files, 0);
    assert_eq!(study.import_report.directories(), 0);
}

#[test]
fn directory_hints_do_not_override_the_objects_own_instance_class_syntax_study_or_series() {
    for case in 0..5 {
        let temp = Temp::new();
        let (bytes, _) = media::directory(&media::records(), &[0, 1, 2, 3, 4], false);
        let path = write_media(&temp.0, &bytes);
        let mut object = dicom_object::from_reader(Cursor::new(image(1))).unwrap();
        match case {
            0 => object.meta_mut().media_storage_sop_instance_uid = "2.25.999".into(),
            1 => {
                object.put_str(Tag(8, 0x16), VR::UI, "1.2.840.10008.5.1.4.1.1.4");
            }
            2 => object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.2".into(),
            3 => {
                object.put_str(Tag(0x20, 0xd), VR::UI, "2.25.999");
            }
            4 => {
                object.put_str(Tag(0x20, 0xe), VR::UI, "2.25.999");
            }
            _ => unreachable!(),
        }
        object.meta_mut().update_information_group_length();
        object.write_to_file(temp.0.join("Images/Img1")).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        assert_eq!(study.frame_count(), 1, "case {case}");
        assert_eq!(
            study.import_report.count(SkipReason::ConflictingReference),
            1,
            "case {case}"
        );
    }
}

#[test]
fn directory_hierarchy_depth_and_archive_namespace_are_bounded() {
    let mut records = media::records();
    records[3].lower = Some(5);
    for i in 5..22 {
        let mut record = records[3].clone();
        record.kind = "PRIVATE";
        record.file = None;
        record.next = None;
        record.lower = (i < 21).then_some(i + 1);
        records.push(record);
    }
    let (bytes, _) = media::directory(&records, &(0..records.len()).collect::<Vec<_>>(), true);
    assert!(matches!(parse(&bytes), Err(LoadError::Limit)));
    let (bytes, _) = media::directory(&media::records(), &[0, 1, 2, 3, 4], true);
    for unsafe_name in ["../SECRET", "C:/SECRET", "IMAGES/../SECRET", "IMAGES\\IMG1"] {
        let temp = Temp::new();
        let path = temp.0.join("generated.zip");
        let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
        for (name, content) in [("DICOMDIR", bytes.clone()), (unsafe_name, image(1))] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(&content).unwrap();
        }
        archive.finish().unwrap();
        assert_eq!(
            index_paths(&[path], || false, |_| {}).unwrap_err(),
            LoadError::Directory
        );
    }
    let temp = Temp::new();
    let path = temp.0.join("linked.zip");
    let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
    archive
        .start_file("DICOMDIR", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(&bytes).unwrap();
    archive
        .add_symlink(
            "IMAGES/IMG1",
            "../../outside",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.finish().unwrap();
    assert_eq!(
        index_paths(&[path], || false, |_| {}).unwrap_err(),
        LoadError::Directory
    );
}

#[test]
fn catalogue_study_groups_are_generated_and_do_not_materialize_pixels_or_merge_unknowns() {
    let temp = Temp::new();
    for (name, study, series) in [
        ("a", "2.25.1", "2.25.11"),
        ("b", "2.25.1", "2.25.12"),
        ("c", "2.25.2", "2.25.11"),
    ] {
        let mut object = dicom_object::from_reader(Cursor::new(image(1))).unwrap();
        object.put_str(Tag(0x20, 0xd), VR::UI, study);
        object.put_str(Tag(0x20, 0xe), VR::UI, series);
        object.write_to_file(temp.0.join(name)).unwrap();
    }
    let mut study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    study
        .series
        .push(crate::study::demo_study().series.remove(0));
    assert_eq!(
        crate::inspection::study_groups(&study, || false).unwrap(),
        vec![Some(1), Some(1), Some(2), None]
    );
    assert_eq!(
        crate::inspection::study_groups(&study, || true).unwrap_err(),
        LoadError::Cancelled
    );
    for series in &study.series[..3] {
        assert_eq!(
            series
                .source
                .as_ref()
                .unwrap()
                .lock_cache(&|| false)
                .unwrap()
                .bytes,
            0
        );
    }
}

#[cfg(unix)]
#[test]
fn retained_directory_capability_rejects_links_replacement_and_parent_redirection_on_later_reads() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let outside = Temp::new();
    let (bytes, _) = media::directory(&media::records(), &[0, 1, 2, 3, 4], true);
    let path = write_media(&temp.0, &bytes);
    write_media(&outside.0, &bytes);
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    fs::rename(temp.0.join("Images"), temp.0.join("Saved")).unwrap();
    symlink(outside.0.join("Images"), temp.0.join("Images")).unwrap();
    assert_eq!(
        study.series[0].read_frame(0, || false).unwrap_err(),
        LoadError::Changed
    );
    assert_eq!(
        index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap_err(),
        LoadError::Directory
    );
    fs::remove_file(temp.0.join("Images")).unwrap();
    fs::rename(temp.0.join("Saved"), temp.0.join("Images")).unwrap();
    // Replacing the image with a fresh inode also fails even if its DICOM IDs agree.
    let replacement = temp.0.join("NEW");
    fs::write(&replacement, image(1)).unwrap();
    fs::rename(replacement, temp.0.join("Images/Img1")).unwrap();
    assert_eq!(
        study.series[0].read_frame(0, || false).unwrap_err(),
        LoadError::Changed
    );
    // A whole file-set moved after opening remains bound to the original handle.
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let moved = temp.0.with_extension("moved");
    fs::rename(&temp.0, &moved).unwrap();
    symlink(&outside.0, &temp.0).unwrap();
    assert_eq!(study.series[0].read_frame(0, || false).unwrap().instance, 1);
    fs::remove_file(&temp.0).unwrap();
    fs::rename(moved, &temp.0).unwrap();
}
