// SPDX-License-Identifier: GPL-3.0-or-later
use super::header::read_header;
use super::*;
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
use std::{
    io::{Seek, SeekFrom, Write},
    sync::atomic::{AtomicBool, Ordering},
};
mod color;
mod formats;
mod large;
mod nifti;
mod nifti_expanded;
mod padding;
mod resources;
use crate::test_support as support;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-index-synthetic-{}-{nonce}-{sequence}",
            std::process::id()
        ));
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
fn indexed_native_and_zip_images_match_eager_decode_and_sort_spatially() {
    for compression in [
        None,
        Some(zip::CompressionMethod::Stored),
        Some(zip::CompressionMethod::Deflated),
    ] {
        let temp = Temp::new();
        let paths = if let Some(compression) = compression {
            let path = temp.0.join("synthetic.zip");
            let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
            for (name, z, instance) in [("a.dcm", 4., 1), ("b.dcm", 2., 9)] {
                zip.start_file(
                    name,
                    zip::write::SimpleFileOptions::default().compression_method(compression),
                )
                .unwrap();
                zip.write_all(&support::fixture(z, instance, false, false))
                    .unwrap();
            }
            zip.finish().unwrap();
            vec![path]
        } else {
            for (name, z, instance) in [("a.dcm", 4., 1), ("b.dcm", 2., 9)] {
                fs::write(
                    temp.0.join(name),
                    support::fixture(z, instance, false, false),
                )
                .unwrap();
            }
            vec![temp.0.clone()]
        };
        let study = index_paths(&paths, || false, |_| {}).unwrap();
        assert_eq!(study.frame_count(), 2);
        let series = &study.series[0];
        assert!(
            series.frames.is_empty(),
            "Indexing must not retain decoded study pixels"
        );
        assert_eq!(series.frame_count(), 2);
        for (i, z, instance) in [(0, 2., 9), (1, 4., 1)] {
            let frame = series.read_frame(i, || false).unwrap();
            let expected =
                crate::load::decode(&support::fixture(z, instance, false, false)).unwrap();
            assert_eq!(frame.pixels, expected.frames[0].pixels);
            assert_eq!(frame.geometry, expected.frames[0].geometry);
            assert_eq!(frame.instance, instance);
        }
        let a = series.read_frame(0, || false).unwrap();
        let b = series.read_frame(0, || false).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!format!("{study:?}").contains("synthetic.zip"));
    }
}

#[test]
fn indexed_sources_reject_mutation_traversal_and_cancellation() {
    let temp = Temp::new();
    let path = temp.0.join("synthetic.dcm");
    fs::write(&path, support::fixture(0., 1, false, false)).unwrap();
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    assert_eq!(
        study.series[0].read_frame(0, || true).unwrap_err(),
        LoadError::Cancelled
    );
    let mut file = File::options().append(true).open(&path).unwrap();
    file.write_all(b"changed").unwrap();
    assert_eq!(
        study.series[0].read_frame(0, || false).unwrap_err(),
        LoadError::Changed
    );
    assert_eq!(
        index_paths(&[temp.0.join("does-not-exist")], || true, |_| {}).unwrap_err(),
        LoadError::Cancelled
    );
    let path = temp.0.join("traversal.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    zip.start_file("../escape.dcm", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&support::fixture(0., 1, false, false))
        .unwrap();
    zip.finish().unwrap();
    assert_eq!(
        index_paths(&[path], || false, |_| {}).unwrap_err(),
        LoadError::Unsupported
    );
    assert!(!temp.0.join("escape.dcm").exists());
    let cancel = AtomicBool::new(false);
    assert_eq!(
        index_paths(
            std::slice::from_ref(&temp.0),
            || {
                cancel.store(true, Ordering::Relaxed);
                true
            },
            |_| {}
        )
        .unwrap_err(),
        LoadError::Cancelled
    );
}

#[test]
fn four_gib_native_multiframe_is_lazy_random_access_and_cache_bounded() {
    let temp = Temp::new();
    let (path, _, _) = large::native(&temp.0);
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    let series = &study.series[0];
    assert_eq!(series.frame_count(), 8191);
    assert!(series.frames.is_empty());
    assert_eq!(series.source.as_ref().unwrap().cache_bytes(), Some(0));
    let first = series.read_frame(0, || false).unwrap();
    assert_eq!(&first.pixels[..4], &[-100., 0., 100., -102.]);
    assert!(
        first.geometry.is_none(),
        "Enhanced geometry must never be fabricated"
    );
    let weak = Arc::downgrade(&first);
    drop(first);
    let last = series.read_frame(8190, || false).unwrap();
    assert_eq!(&last.pixels[..4], &[100., 0., -100., -102.]);
    assert_eq!(last.frame_index, 8190);
    assert_eq!(
        series.volume_frames(|| false).unwrap_err(),
        LoadError::Limit
    );
    for i in 1..=130 {
        series.read_frame(i, || false).unwrap();
    }
    assert!(
        weak.upgrade().is_none(),
        "Eviction releases old pixel payloads"
    );
    assert!(series.source.as_ref().unwrap().cache_bytes().unwrap() <= CACHE_BYTES);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert!(fs::metadata(path).unwrap().blocks() * 512 < 8 * 1024 * 1024);
    }
}

#[test]
fn sparse_zip64_above_four_gib_streams_crc_and_reads_selected_frames() {
    let temp = Temp::new();
    let (native, header, bytes) = large::native(&temp.0);
    let zip = large::zip64(&temp.0, &native, &header, bytes);
    assert!(fs::metadata(&zip).unwrap().len() > 1u64 << 32);
    let mut progress = Progress::default();
    let study = index_paths(std::slice::from_ref(&zip), || false, |p| progress = p).unwrap();
    assert_eq!(study.frame_count(), 8191);
    assert_eq!(progress.files, 1);
    assert_eq!(progress.checked_bytes, header.len() as u64 + bytes);
    assert_eq!(
        &study.series[0].read_frame(8190, || false).unwrap().pixels[..4],
        &[100., 0., -100., -102.]
    );
    assert_eq!(
        study.series[0].source.as_ref().unwrap().cache_bytes(),
        Some(512 * 512 * 4)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert!(fs::metadata(zip).unwrap().blocks() * 512 < 8 * 1024 * 1024);
    }
}

#[test]
fn big_endian_native_frame_uses_the_same_signed_modality_values() {
    let temp = Temp::new();
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, true, false)))
            .unwrap();
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.2".into();
    let path = temp.0.join("big-endian.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let frame = study.series[0].read_frame(0, || false).unwrap();
    assert_eq!(&frame.pixels[..4], &[-100., 0., 100., -102.]);
    assert!(frame.monochrome1);
}
