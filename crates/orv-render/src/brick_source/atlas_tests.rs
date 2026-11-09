// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn verify(probe: &Probe, atlas: &Arc<GpuBrickAtlas>, points: &[[f32; 4]]) -> usize {
    let values = probe.run_bindings(atlas.bindings(), points);
    let resident = atlas.resident_indices().collect::<Vec<_>>();
    let mut known = 0;
    for (point, v) in points.iter().zip(values) {
        let p = [point[0] as f64, point[1] as f64, point[2] as f64];
        let expected = match probe.source.layout.owner(p) {
            None => BrickSampleStatus::OutsideSource as u32,
            Some(i) if !resident.contains(&i) => BrickSampleStatus::Nonresident as u32,
            _ => {
                if known_sample(&probe.source.source, p) {
                    BrickSampleStatus::Known as u32
                } else {
                    BrickSampleStatus::Missing as u32
                }
            }
        };
        assert_eq!(v[1], expected, "atlas sample {p:?}");
        if expected == BrickSampleStatus::Known as u32 {
            assert_eq!(v[7], expected);
            assert_eq!(v[0], v[6], "atlas scalar {p:?}");
            known += 1;
        }
        let expected_gradient = if expected != BrickSampleStatus::Known as u32 {
            expected
        } else if probe.source.layout.stencil() == BrickStencil::Trilinear {
            BrickSampleStatus::StencilUnavailable as u32
        } else {
            v[11]
        };
        assert_eq!(v[5], expected_gradient, "atlas gradient status {p:?}");
        if expected_gradient == BrickSampleStatus::Known as u32 {
            assert_eq!(v[2..5], v[8..11], "atlas gradient {p:?}");
        }
    }
    known
}
#[test]
fn immutable_atlas_samples_match_dense_before_and_after_slot_replacement() {
    let (d, q) = device(None);
    let mut known = 0;
    for side in [32, 64, 128] {
        for (kind, missing, reverse) in [
            (0, false, false),
            (0, true, true),
            (1, false, true),
            (2, true, false),
            (3, false, false),
        ] {
            let cpu = source([side * 2 + 5, 39, 35], kind, missing, reverse);
            let source =
                GpuBrickSource::new(&d, &q, cpu, side, BrickStencil::CentralSecants, || false)
                    .unwrap();
            let pool = GpuBrickAtlasPool::new(source.clone(), 2, || false).unwrap();
            let probe = Probe::for_atlas(&pool);
            let empty = GpuBrickResidency::prepare(source.clone(), &[], None, 0, || false).unwrap();
            let blank = GpuBrickAtlas::prepare(pool.clone(), &empty, None, || false).unwrap();
            let p = points(&source, 0);
            assert_eq!(verify(&probe, &blank, &p), 0);
            let old = GpuBrickResidency::prepare(
                source.clone(),
                &[0, 1],
                Some(&empty),
                MAX_RESIDENT_BRICK_BYTES,
                || false,
            )
            .unwrap();
            let a = GpuBrickAtlas::prepare(pool.clone(), &old, Some(&blank), || false).unwrap();
            assert_eq!(a.statistics().copied_from_pages, 2);
            known += verify(&probe, &a, &p);
            let old_pixels = probe.run_bindings(a.bindings(), &p);
            let next = GpuBrickResidency::prepare(
                source.clone(),
                &[1, 2],
                Some(&old),
                MAX_RESIDENT_BRICK_BYTES,
                || false,
            )
            .unwrap();
            let b = GpuBrickAtlas::prepare(pool.clone(), &next, Some(&a), || false).unwrap();
            assert_eq!(b.statistics().copied_from_pages, 1);
            assert_eq!(b.statistics().copied_from_prior_atlas, 1);
            assert_eq!(
                b.statistics().copied_bytes,
                next.page(1).unwrap().uploaded_bytes() + next.page(2).unwrap().uploaded_bytes()
            );
            assert_eq!(probe.run_bindings(a.bindings(), &p), old_pixels);
            for i in 0..source.layout.len() {
                known += verify(&probe, &b, &points(&source, i));
            }
            let direct = GpuBrickAtlas::load(pool.clone(), &[0, 2], Some(&b), || false).unwrap();
            assert_eq!(direct.statistics().uploaded_pages, 1);
            assert_eq!(direct.statistics().copied_from_prior_atlas, 1);
            assert_eq!(probe.run_bindings(a.bindings(), &p), old_pixels);
            for i in 0..source.layout.len() {
                known += verify(&probe, &direct, &points(&source, i));
            }
            let reused = GpuBrickAtlas::prepare(pool, &next, Some(&b), || false).unwrap();
            assert!(Arc::ptr_eq(&reused, &b));
        }
    }
    assert!(known > 3000);
}
#[test]
fn atlas_capacity_device_limits_cancellation_and_admission_preserve_old_snapshots() {
    let (d, q) = device(Some(64));
    let cpu = source([129, 9, 7], 1, true, false);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    assert!(GpuBrickAtlasPool::new(source.clone(), 0, || false).is_err());
    assert!(GpuBrickAtlasPool::new(source.clone(), 5, || false).is_err());
    assert!(GpuBrickAtlasPool::new(source.clone(), 2, || true).is_err());
    let pool = GpuBrickAtlasPool::new(source.clone(), 2, || false).unwrap();
    assert!(pool.texture_size().into_iter().all(|v| v <= 64));
    let resident = GpuBrickResidency::prepare(
        source.clone(),
        &[0, 1],
        None,
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    )
    .unwrap();
    let old = GpuBrickAtlas::prepare(pool.clone(), &resident, None, || false).unwrap();
    let next = GpuBrickResidency::prepare(
        source.clone(),
        &[2, 3],
        Some(&resident),
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    )
    .unwrap();
    let probe = Probe::for_atlas(&pool);
    let at = points(&source, 0);
    let original = probe.run_bindings(old.bindings(), &at);
    fence(&d);
    let gpu = MemoryBudget::gpu();
    let ram = MemoryBudget::volumes();
    let stable_gpu = gpu.used();
    let stable_cpu = ram.used();
    for cutoff in [0, 1, 2, 3, 4] {
        let calls = Cell::new(0);
        let result = GpuBrickAtlas::prepare(pool.clone(), &next, Some(&old), || {
            let n = calls.get();
            calls.set(n + 1);
            n >= cutoff
        });
        assert!(result.is_err(), "cutoff {cutoff}");
        fence(&d);
        assert_eq!(gpu.used(), stable_gpu);
        assert_eq!(ram.used(), stable_cpu);
        assert_eq!(probe.run_bindings(old.bindings(), &at), original);
        fence(&d);
    }
    let limit = gpu.limit();
    gpu.set_limit(stable_gpu);
    let result = GpuBrickAtlas::prepare(pool.clone(), &next, Some(&old), || false);
    gpu.set_limit(limit);
    assert!(result.is_err());
    let limit = ram.limit();
    ram.set_limit(stable_cpu);
    let result = GpuBrickAtlas::prepare(pool.clone(), &next, Some(&old), || false);
    ram.set_limit(limit);
    assert!(result.is_err());
    fence(&d);
    assert_eq!(gpu.used(), stable_gpu);
    assert_eq!(ram.used(), stable_cpu);
    let too_many = GpuBrickResidency::prepare(
        source.clone(),
        &[0, 1, 2],
        Some(&resident),
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    )
    .unwrap();
    assert!(GpuBrickAtlas::prepare(pool.clone(), &too_many, Some(&old), || false).is_err());
    let other = GpuBrickAtlasPool::new(source.clone(), 2, || false).unwrap();
    assert!(GpuBrickAtlas::prepare(other.clone(), &next, Some(&old), || false).is_err());
    assert_eq!(probe.run_bindings(old.bindings(), &at), original);
    drop((probe, other, too_many, next, old, resident, pool, source));
    fence(&d);
    assert_eq!(gpu.used(), before_gpu);
    assert_eq!(ram.used(), before_cpu);
}

fn known_sample(source: &Volume, point: [f64; 3]) -> bool {
    let dims = source.dimensions();
    let q: [f64; 3] = std::array::from_fn(|a| point[a].clamp(0., (dims[a] - 1) as f64));
    let lo = q.map(|v| v.floor() as usize);
    let hi: [usize; 3] = std::array::from_fn(|a| (lo[a] + 1).min(dims[a] - 1));
    let t: [f64; 3] = std::array::from_fn(|a| q[a] - lo[a] as f64);
    for (z, wz) in [(lo[2], 1. - t[2]), (hi[2], t[2])] {
        for (y, wy) in [(lo[1], 1. - t[1]), (hi[1], t[1])] {
            for (x, wx) in [(lo[0], 1. - t[0]), (hi[0], t[0])] {
                if wx * wy * wz > 0. && !source.frames()[z].has_sample(y * dims[0] + x) {
                    return false;
                }
            }
        }
    }
    true
}

#[test]
fn direct_atlas_upload_failures_after_copy_or_queued_writes_retire_scratch_without_changing_old_data()
 {
    let (d, q) = device(Some(64));
    let cpu = source([129, 9, 7], 0, true, false);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let pool = GpuBrickAtlasPool::new(source.clone(), 2, || false).unwrap();
    let old = GpuBrickAtlas::load(pool.clone(), &[0, 1], None, || false).unwrap();
    let probe = Probe::for_atlas(&pool);
    let at = points(&source, 0);
    let pixels = probe.run_bindings(old.bindings(), &at);
    fence(&d);
    let gpu = MemoryBudget::gpu();
    let ram = MemoryBudget::volumes();
    let stable_gpu = gpu.used();
    let stable_cpu = ram.used();
    for cutoff in [0, 1, 10, 80] {
        let calls = Cell::new(0);
        let result = GpuBrickAtlas::load(pool.clone(), &[2, 3], Some(&old), || {
            let n = calls.get();
            calls.set(n + 1);
            n >= cutoff
        });
        assert!(result.is_err(), "cutoff{cutoff}");
        fence(&d);
        assert_eq!(gpu.used(), stable_gpu);
        assert_eq!(ram.used(), stable_cpu);
        assert_eq!(probe.run_bindings(old.bindings(), &at), pixels);
        fence(&d);
    }
    let limit = gpu.limit();
    gpu.set_limit(stable_gpu + pool.admission_bytes() + 1024);
    let result = GpuBrickAtlas::load(pool.clone(), &[2, 3], Some(&old), || false);
    gpu.set_limit(limit);
    assert!(result.is_err());
    fence(&d);
    assert_eq!(gpu.used(), stable_gpu);
    assert_eq!(ram.used(), stable_cpu);
    let limit = ram.limit();
    ram.set_limit(stable_cpu + 1024);
    let result = GpuBrickAtlas::load(pool.clone(), &[2, 3], Some(&old), || false);
    ram.set_limit(limit);
    assert!(result.is_err());
    fence(&d);
    assert_eq!(gpu.used(), stable_gpu);
    assert_eq!(ram.used(), stable_cpu);
    for ids in [&[0, 0][..], &[4][..], &[usize::MAX][..]] {
        assert!(GpuBrickAtlas::load(pool.clone(), ids, Some(&old), || false).is_err());
    }
    assert_eq!(probe.run_bindings(old.bindings(), &at), pixels);
    drop((probe, old, pool, source));
    fence(&d);
    assert_eq!(gpu.used(), before_gpu);
    assert_eq!(ram.used(), before_cpu);
}

#[test]
fn private_quiescent_cache_unmaps_replaced_slots_before_cancel_or_refused_upload_then_recovers() {
    let (d, q) = device(Some(64));
    let cpu = source([129, 9, 7], 0, true, false);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let pool = GpuBrickAtlasPool::new(source.clone(), 2, || false).unwrap();
    let mut cache = GpuBrickAtlasStream::new(pool.clone(), &|| false).unwrap();
    cache.load(&[0, 1], &|| false).unwrap();
    let probe = Probe::for_atlas(&pool);
    let mut at = points(&source, 0);
    at.extend(points(&source, 2));
    let original = probe.run_bindings(cache.bindings(), &at);
    fence(&d);
    let gpu = MemoryBudget::gpu();
    let ram = MemoryBudget::volumes();
    let stable_gpu = gpu.used();
    let stable_cpu = ram.used();
    for cutoff in [0, 1, 10, 80] {
        let calls = Cell::new(0);
        let result = cache.load(&[2, 3], &|| {
            let n = calls.get();
            calls.set(n + 1);
            n >= cutoff
        });
        assert!(result.is_err(), "cutoff{cutoff}");
        fence(&d);
        assert_eq!(gpu.used(), stable_gpu);
        assert_eq!(ram.used(), stable_cpu);
        let current = probe.run_bindings(cache.bindings(), &at);
        for (p, v) in at.iter().zip(current) {
            let p = [p[0] as f64, p[1] as f64, p[2] as f64];
            let expected = match source.layout().owner(p) {
                None => BrickSampleStatus::OutsideSource as u32,
                Some(i) if !cache.contains(i) => BrickSampleStatus::Nonresident as u32,
                _ => v[7],
            };
            assert_eq!(v[1], expected, "unmapped cache {p:?}");
            if expected == BrickSampleStatus::Known as u32 {
                assert_eq!(v[0], v[6]);
                assert_eq!(v[2..5], v[8..11]);
            }
        }
        cache.load(&[0, 1], &|| false).unwrap();
        assert_eq!(probe.run_bindings(cache.bindings(), &at), original);
        fence(&d);
        assert_eq!(gpu.used(), stable_gpu);
        assert_eq!(ram.used(), stable_cpu);
    }
    let limit = gpu.limit();
    gpu.set_limit(stable_gpu + (source.layout().len() * 32 * 2) as u64);
    let result = cache.load(&[2, 3], &|| false);
    gpu.set_limit(limit);
    assert!(result.is_err());
    fence(&d);
    assert_eq!(gpu.used(), stable_gpu);
    assert_eq!(ram.used(), stable_cpu);
    cache.load(&[0, 1], &|| false).unwrap();
    assert_eq!(probe.run_bindings(cache.bindings(), &at), original);
    fence(&d);
    let limit = ram.limit();
    ram.set_limit(stable_cpu + 512);
    let result = cache.load(&[2, 3], &|| false);
    ram.set_limit(limit);
    assert!(result.is_err());
    fence(&d);
    assert_eq!(gpu.used(), stable_gpu);
    assert_eq!(ram.used(), stable_cpu);
    cache.load(&[0, 1], &|| false).unwrap();
    assert_eq!(probe.run_bindings(cache.bindings(), &at), original);
    assert_eq!(
        cache.load(&[0, 1], &|| false).unwrap(),
        AtlasCopyStatistics::default()
    );
    drop((probe, cache, pool, source));
    fence(&d);
    assert_eq!(gpu.used(), before_gpu);
    assert_eq!(ram.used(), before_cpu);
}
