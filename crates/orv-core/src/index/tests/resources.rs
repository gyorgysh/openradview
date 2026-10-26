// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn resident_volume_reuse_checks_immutable_frame_identity() {
    let mut series = crate::load::decode(&support::fixture(0., 0, false, false)).unwrap();
    series.frames.extend(
        crate::load::decode(&support::fixture(1., 1, false, false))
            .unwrap()
            .frames,
    );
    let first = series.volume(|| false).unwrap();
    assert!(Arc::ptr_eq(&first, &series.volume(|| false).unwrap()));
    let mut changed = series.clone();
    let mut pixels = changed.frames[0].pixels.to_vec();
    pixels[0] += 1.;
    Arc::make_mut(&mut changed.frames[0]).pixels = pixels.into();
    let second = changed.volume(|| false).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_ne!(
        first.source_frames()[0].pixels[0],
        second.source_frames()[0].pixels[0]
    );
    assert!(series.volume(|| true).is_err());
}

#[test]
fn prepared_volumes_share_pixels_and_one_reservation_without_a_persistent_owner() {
    let temp = Temp::new();
    for (name, z) in [("a", 0.), ("b", 1.)] {
        fs::write(temp.0.join(name), support::fixture(z, 1, false, false)).unwrap();
    }
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    let series = &study.series[0];
    let source = series.source.as_ref().unwrap();
    let reimport = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    assert!(Arc::ptr_eq(
        &source.budget,
        &reimport.series[0].source.as_ref().unwrap().budget
    ));
    let first = series.volume(|| false).unwrap();
    let retained = source.budget.used();
    assert_eq!(retained, source.decoded_bytes());
    let second = series.volume(|| false).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(source.budget.used(), retained);
    let weak = Arc::downgrade(&first);
    drop(first);
    drop(second);
    assert!(weak.upgrade().is_none());
    assert_eq!(source.budget.used(), 0);
    assert!(series.volume(|| true).is_err());
    assert_eq!(source.budget.used(), 0);
    let polls = std::cell::Cell::new(0);
    assert!(
        series
            .volume(|| {
                polls.set(polls.get() + 1);
                polls.get() > 5
            })
            .is_err()
    );
    assert_eq!(source.budget.used(), 0);
}

#[test]
fn two_series_cannot_each_take_the_whole_study_volume_allowance() {
    let temp = Temp::new();
    for series in 0..2 {
        for z in 0..2 {
            let mut object = dicom_object::from_reader(std::io::Cursor::new(support::fixture(
                z as f64, z, false, false,
            )))
            .unwrap();
            object.put_str(
                dicom_core::Tag(0x0020, 0x000e),
                dicom_core::VR::UI,
                format!("2.25.100{series}"),
            );
            let mut bytes = Vec::new();
            object.write_all(&mut bytes).unwrap();
            fs::write(temp.0.join(format!("{series}-{z}")), bytes).unwrap();
        }
    }
    let mut study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    assert_eq!(study.series.len(), 2);
    let bytes = study.series[0].source.as_ref().unwrap().decoded_bytes();
    let budget = MemoryBudget::new(bytes * 2 - 1);
    for series in &mut study.series {
        Arc::get_mut(series.source.as_mut().unwrap())
            .unwrap()
            .budget = budget.clone();
    }
    let first = study.series[0].volume(|| false).unwrap();
    assert!(study.series[1].volume(|| false).is_err());
    assert_eq!(budget.used(), bytes);
    drop(first);
    assert!(study.series[1].volume(|| false).is_ok());
    assert_eq!(budget.used(), 0);
}

#[test]
fn decoded_cache_is_shared_across_imports_evicts_and_releases_without_history() {
    let temp = Temp::new();
    for z in 0..3 {
        fs::write(
            temp.0.join(z.to_string()),
            support::fixture(z as f64, z, false, false),
        )
        .unwrap();
    }
    let first = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    let second = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    let a = first.series[0].source.as_ref().unwrap();
    let b = second.series[0].source.as_ref().unwrap();
    assert!(Arc::ptr_eq(
        &a.cache.lock().unwrap().budget,
        &b.cache.lock().unwrap().budget
    ));
    let bytes = 2 * 2 * 4;
    let budget = MemoryBudget::new(2 * bytes);
    a.cache.lock().unwrap().budget = budget.clone();
    b.cache.lock().unwrap().budget = budget.clone();
    let image = a.read_frame(0, || false).unwrap();
    let weak_a = Arc::downgrade(&image);
    drop(image);
    let image = b.read_frame(0, || false).unwrap();
    let weak_b = Arc::downgrade(&image);
    drop(image);
    assert_eq!(budget.used(), 2 * bytes);
    drop(a.read_frame(1, || false).unwrap());
    assert!(
        weak_a.upgrade().is_none(),
        "Eviction releases app-owned cache pixels"
    );
    assert_eq!(budget.used(), 2 * bytes);
    budget.set_limit(0);
    assert_eq!(a.read_frame(2, || false).unwrap().pixels.len(), 4);
    assert_eq!(
        a.cache_bytes(),
        Some(0),
        "2D still works with no cache allowance"
    );
    assert_eq!(budget.used(), bytes);
    drop(second);
    assert!(weak_b.upgrade().is_none());
    assert_eq!(budget.used(), 0);
    drop(first);
    assert_eq!(budget.used(), 0);
}
