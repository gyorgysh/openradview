// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn fixed_cells_preserve_raster_aspects_and_have_independent_known_positions() {
    let layout = Layout::new(&[[100, 50], [40, 100], [80, 80]], 2).unwrap();
    assert_eq!(layout.size(), [208, 208]);
    assert_eq!(
        layout.tiles(),
        &[
            Tile {
                origin: [0, 25],
                size: [100, 50]
            },
            Tile {
                origin: [138, 0],
                size: [40, 100]
            },
            Tile {
                origin: [10, 118],
                size: [80, 80]
            },
        ]
    );
    let layout = Layout::new(&[[1024, 1024]; 16], 4).unwrap();
    assert_eq!(layout.size(), [2048, 2048]);
    assert_eq!(
        layout.tiles()[15],
        Tile {
            origin: [1542, 1542],
            size: [506, 506]
        }
    );
    for columns in 1..=4 {
        let layout = Layout::new(&[[1, 1024]; 16], columns).unwrap();
        assert!(layout.size().iter().all(|n| (1..=2048).contains(n)));
        assert!(layout.tiles().iter().all(|tile| tile.size[0] == 1));
    }
    let serialized = serde_json::to_value(layout).unwrap();
    assert_eq!(serialized["columns"], 4);
    assert_eq!(serialized["tiles"].as_array().unwrap().len(), 16);
}

#[test]
fn unbounded_empty_or_unoccupied_layouts_fail_before_image_allocation() {
    for sizes in [
        vec![],
        vec![[1, 1]; 17],
        vec![[0, 1]],
        vec![[1025, 1]],
        vec![[1, u32::MAX]],
    ] {
        assert!(Layout::new(&sizes, 1).is_err());
    }
    for columns in [0, 2, 5, u32::MAX] {
        assert!(Layout::new(&[[10, 10]], columns).is_err());
    }
    assert!(Layout::new(&[[10, 10]; 5], 5).is_err());
}

#[test]
fn mosaic_pressure_and_cancel_release_all_admission_before_publishing_pixels() {
    let layout = || Layout::new(&[[1024, 1024]; 16], 4).unwrap();
    let required = layout().rgba_bytes() as u64 + OVERHEAD;
    let budget = MemoryBudget::new(required - 1);
    assert!(Mosaic::with_budget(layout(), &budget, || false).is_err());
    assert_eq!(budget.used(), 0);
    budget.set_limit(required);
    let calls = std::cell::Cell::new(0);
    assert!(
        Mosaic::with_budget(layout(), &budget, || {
            let n = calls.get();
            calls.set(n + 1);
            n == 3
        })
        .is_err()
    );
    assert_eq!(budget.used(), 0);
    let mosaic = Mosaic::with_budget(layout(), &budget, || false).unwrap();
    assert_eq!(budget.used(), required);
    assert!(
        mosaic
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [10, 10, 10, 255])
    );
    drop(mosaic);
    assert_eq!(budget.used(), 0);
}

#[test]
fn known_cells_gutters_and_empty_slot_encode_only_after_every_tile_is_complete() {
    let layout = Layout::new(&[[3, 2], [1, 3], [2, 2]], 2).unwrap();
    assert_eq!(layout.size(), [14, 14]);
    let mut mosaic = Mosaic::new(layout, || false).unwrap();
    assert!(mosaic.png(|| false).is_err());
    assert!(mosaic.copy_tile(3, &[], || false).is_err());
    assert!(mosaic.copy_tile(0, &[0; 4], || false).is_err());
    for (index, color) in [[240, 0, 0, 255], [0, 180, 0, 255], [0, 0, 120, 255]]
        .into_iter()
        .enumerate()
    {
        let tile = mosaic.layout().tiles()[index];
        let pixels = color.repeat((tile.size[0] * tile.size[1]) as usize);
        mosaic.copy_tile(index, &pixels, || false).unwrap();
        assert!(mosaic.copy_tile(index, &pixels, || false).is_err());
    }
    let png = mosaic.png(|| false).unwrap();
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (14, 14));
    assert_eq!(image.as_raw(), mosaic.rgba());
    assert_eq!(image.get_pixel(0, 0).0, [240, 0, 0, 255]);
    assert_eq!(image.get_pixel(12, 2).0, [0, 180, 0, 255]);
    assert_eq!(image.get_pixel(1, 12).0, [0, 0, 120, 255]);
    for point in [(3, 0), (0, 3), (5, 5), (12, 12)] {
        assert_eq!(image.get_pixel(point.0, point.1).0, [10, 10, 10, 255]);
    }
    assert!(mosaic.png(|| true).is_err());
}

#[test]
fn cancelled_partial_tile_is_incomplete_and_a_retry_replaces_every_row() {
    let mut mosaic = Mosaic::new(Layout::new(&[[4, 4]], 1).unwrap(), || false).unwrap();
    let pixels = [25, 50, 75, 255].repeat(16);
    let calls = std::cell::Cell::new(0);
    assert!(
        mosaic
            .copy_tile(0, &pixels, || {
                let n = calls.get();
                calls.set(n + 1);
                n == 2
            })
            .is_err()
    );
    assert_eq!(mosaic.completed, 0);
    assert_eq!(&mosaic.rgba()[..4], &[25, 50, 75, 255]);
    assert_eq!(&mosaic.rgba()[48..52], &[10, 10, 10, 255]);
    assert!(mosaic.png(|| false).is_err());
    mosaic
        .copy_tile(0, &[90, 80, 70, 255].repeat(16), || false)
        .unwrap();
    assert!(
        mosaic
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [90, 80, 70, 255])
    );
    mosaic.png(|| false).unwrap();
}

#[test]
fn full_2048_square_mosaic_round_trips_incompressible_pixels_inside_the_png_bound() {
    let mut mosaic = Mosaic::new(Layout::new(&[[1024, 1024]; 16], 4).unwrap(), || false).unwrap();
    let mut seed = 0x1a2b3c4du32;
    for index in 0..16 {
        let tile = mosaic.layout().tiles()[index];
        let pixels: Vec<u8> = (0..tile.size[0] as usize * tile.size[1] as usize * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        mosaic.copy_tile(index, &pixels, || false).unwrap();
    }
    let png = mosaic.png(|| false).unwrap();
    assert!(png.len() > 8 * 1024 * 1024 && png.len() <= mosaic.layout().png_limit());
    let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(decoded.dimensions(), (2048, 2048));
    assert_eq!(decoded.as_raw(), mosaic.rgba());
}
