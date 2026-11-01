// SPDX-License-Identifier: GPL-3.0-or-later
//! One decoded/readback tile at a time; only bounded numeric metadata accumulates.
use super::*;
use crate::export::illustration::sheets::{Settings, Sheet};
use orv_render::snapshot::mosaic::{Layout, Mosaic};

#[derive(Serialize)]
struct TileRecord<'a> {
    caption: Caption,
    scene: &'a SceneState,
    image: ImageMetadata,
    marks: Option<&'a [crate::annotations::Outline]>,
    masks: Option<MaskRecord<'a>>,
}
#[derive(Serialize)]
struct SheetManifest<'a> {
    format: &'static str,
    schema_version: u32,
    renderer_version: u32,
    base_renderer_version: u32,
    app_version: &'static str,
    source_sha256: &'a str,
    png_sha256: String,
    layers: Choice,
    outline_style: Option<OutlineStyle>,
    caption_style: &'static str,
    layout: &'a Layout,
    tiles: &'a [serde_json::Value],
}
fn admission(layout: &Layout, budget: &Arc<MemoryBudget>) -> Result<Reservation, &'static str> {
    budget
        .reserve(layout.rgba_bytes() as u64 * 4 + 3 * MIB + 16 * 1024)
        .ok_or("Not enough memory for the sheet preview")
}
pub(super) fn prepare(
    request: Request,
    cancel: &dyn Fn() -> bool,
) -> Result<Arc<Artifact>, String> {
    let input = &request.input;
    input.base.scene.validate()?;
    let settings = request
        .choice
        .sheet
        .ok_or("Choose a captured sheet layout")?;
    let sheet = input
        .sheet
        .as_ref()
        .ok_or("No captured sheet is available")?;
    let (layout, ordinals) = sheet.layout(settings)?;
    match sheet {
        Sheet::Source { total, .. }
            if *total != input.base.series.frame_count()
                || input.base.scene.view.plane != orv_core::Plane::Acquired
                || input.base.scene.crosshair_mm.is_some()
                || input.base.scene.reconstruction.is_some()
                || !matches!(input.base.pixels, Pixels::Image(_)) =>
        {
            return Err("The captured source sheet changed".into());
        }
        Sheet::Mpr { panes } => validate_mpr(input, panes, settings)?,
        _ => {}
    }
    let size = layout.size();
    let memory = admission(&layout, &MemoryBudget::volumes())?;
    // Coordinates are bounded by the original collections (128/4096 per pane).
    // JSON trees, temporary per-tile encoding and the final metadata share this
    // working allowance; both individual and aggregate byte writers stay capped.
    let _metadata = MemoryBudget::volumes()
        .reserve(8 * MIB)
        .ok_or("Not enough memory for sheet settings")?;
    let tile_memory = MemoryBudget::volumes()
        .reserve(4 * MIB)
        .ok_or("Not enough memory for streamed tile pixels")?;
    let source_sha256 = verify_source(input, request.choice.masks, cancel)?;
    let mosaic = Mosaic::new(layout, cancel)?;
    let mut records = Vec::new();
    records
        .try_reserve_exact(mosaic.layout().tiles().len())
        .map_err(|_| "Not enough memory for sheet settings")?;
    let mut writer = SheetWriter {
        mosaic,
        records,
        metadata_bytes: 0,
        renderer: SnapshotRenderer::with_device(
            input.base.device.clone(),
            input.base.queue.clone(),
        ),
    };

    match sheet {
        Sheet::Source { marks, total } => {
            if *total != input.base.series.frame_count()
                || !matches!(input.base.pixels, Pixels::Image(_))
            {
                return Err("The captured source sheet changed".into());
            }
            for (index, ordinal) in ordinals.iter().copied().enumerate() {
                if cancel() {
                    return Err("Illustration cancelled".into());
                }
                let image = input
                    .base
                    .series
                    .read_fresh_frame(ordinal as usize, cancel)
                    .map_err(|_| "A contact image is unavailable or changed")?;
                let mut scene = input.base.scene.clone();
                scene.view.slice = ordinal;
                scene.output_size = writer.mosaic.layout().tiles()[index].size;
                scene.source_sha256 = source_sha256.clone();
                let outlines = marks.for_frame(&image, &scene.view, &cancel)?;
                let tile = FrozenInput {
                    base: Input {
                        scene,
                        pixels: Pixels::Image(image),
                        ..input.base.clone()
                    },
                    outlines,
                    masks: input.masks.clone(),
                    mask_note: input.mask_note,
                    sheet: None,
                };
                writer.record(
                    &tile,
                    &tile.base.scene,
                    request.choice,
                    Caption::Source(ordinal + 1),
                    cancel,
                )?;
            }
        }
        Sheet::Mpr { panes } => {
            for (index, (pane, caption)) in panes
                .iter()
                .zip([Caption::Axial, Caption::Coronal, Caption::Sagittal])
                .enumerate()
            {
                let mut scene = pane.base.scene.clone();
                scene.output_size = writer.mosaic.layout().tiles()[index].size;
                scene.source_sha256 = source_sha256.clone();
                writer.record(pane, &scene, request.choice, caption, cancel)?;
            }
        }
    }
    drop(tile_memory);
    let png = writer.mosaic.png(cancel)?;
    let manifest = SheetManifest {
        format: "openradview_illustration",
        schema_version: 2,
        renderer_version: 2,
        base_renderer_version: orv_render::snapshot::RENDERER_VERSION,
        app_version: env!("CARGO_PKG_VERSION"),
        source_sha256: &source_sha256,
        png_sha256: hash(&png, cancel)?,
        layers: request.choice,
        outline_style: request.choice.marks.then(outline_style),
        caption_style: "bundled IBM Plex Mono 11px; 8px inset; opaque dark background with 2px padding",
        layout: writer.mosaic.layout(),
        tiles: &writer.records,
    };
    let mut json = LimitedBytes { bytes: Vec::new() };
    serde_json::to_writer_pretty(&mut json, &manifest)
        .map_err(|_| "Could not encode bounded sheet settings")?;
    drop(writer);
    finish(
        request,
        source_sha256,
        Rendered {
            png,
            manifest: json.bytes,
            size,
        },
        memory,
        cancel,
    )
}
fn validate_mpr(
    input: &FrozenInput,
    panes: &[FrozenInput],
    settings: Settings,
) -> Result<(), &'static str> {
    if panes.len() != 3 || settings.range.is_some() {
        return Err("The captured MPR grid is incomplete");
    }
    let Pixels::Reformat(_, volume) = &input.base.pixels else {
        return Err("MPR sheets need a captured volume");
    };
    let point = input
        .base
        .scene
        .crosshair_mm
        .ok_or("The captured MPR point is missing")?;
    for (pane, plane) in panes.iter().zip([
        orv_core::Plane::Axial,
        orv_core::Plane::Coronal,
        orv_core::Plane::Sagittal,
    ]) {
        pane.base.scene.validate()?;
        pane.image()?;
        let matching_masks = match (&input.masks, &pane.masks) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if !matching_masks
            || !Arc::ptr_eq(&input.base.series, &pane.base.series)
            || pane.base.scene.crosshair_mm != Some(point)
            || pane.base.scene.view.plane != plane
            || pane.base.scene.reconstruction.is_some()
            || pane.base.scene.reformat_side != input.base.scene.reformat_side
            || !matches!(&pane.base.pixels, Pixels::Reformat(_, source) if Arc::ptr_eq(source,volume))
        {
            return Err("MPR sheets need three coherent planes from the same captured source");
        }
    }
    Ok(())
}
struct SheetWriter {
    renderer: SnapshotRenderer,
    mosaic: Mosaic,
    records: Vec<serde_json::Value>,
    metadata_bytes: usize,
}
impl SheetWriter {
    fn record(
        &mut self,
        input: &FrozenInput,
        scene: &SceneState,
        choice: Choice,
        caption: Caption,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        scene.validate()?;
        let image = input.image()?;
        input.outlines.validate(image, &scene.view, &cancel)?;
        if choice.masks && input.masks.is_none() {
            return Err("No applied masks are available for this sheet".into());
        }
        let rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(scene.output_size[0] as f32, scene.output_size[1] as f32),
        );
        if choice.marks {
            input
                .outlines
                .check_drawing_size(rect, image, &scene.view)?;
        }
        let target = self.renderer.render_slice_image(image, scene, cancel)?;
        // Painting uses the tile-sized scene, including its new raster aspect.
        let (composition, mask) = compose(input, scene, choice, &target, Some(caption), cancel)?;
        let rgba = target.rgba(cancel)?;
        self.mosaic.copy_tile(self.records.len(), &rgba, cancel)?;
        let record = TileRecord {
            caption,
            scene,
            image: ImageMetadata::from_frame(image),
            marks: choice.marks.then(|| input.outlines.marks()),
            masks: mask,
        };
        let mut json = LimitedBytes { bytes: Vec::new() };
        serde_json::to_writer(&mut json, &record)
            .map_err(|_| "Could not encode bounded tile settings")?;
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(json.bytes.len())
            .filter(|n| *n <= MANIFEST_LIMIT)
            .ok_or("Sheet settings exceed their allowance")?;
        self.records.push(
            serde_json::from_slice(&json.bytes).map_err(|_| "Could not retain tile settings")?,
        );
        drop(composition);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sheet_output_admission_is_separate_from_single_pane_caps_and_releases_on_failure() {
        let layout = Layout::new(&[[1024, 1024]; 16], 4).unwrap();
        let required = 4 * 2048 * 2048 * 4 + 3 * MIB + 16 * 1024;
        let budget = MemoryBudget::new(required - 1);
        assert!(admission(&layout, &budget).is_err());
        assert_eq!(budget.used(), 0);
        budget.set_limit(required);
        let memory = admission(&layout, &budget).unwrap();
        assert_eq!(budget.used(), required);
        assert!(artifact_admission([2048, 2048], &budget).is_err());
        drop(memory);
        assert_eq!(budget.used(), 0);
    }
    #[cfg(feature = "gpu-tests")]
    #[test]
    fn real_gpu_source_sheet_uses_requested_ordinals_masks_and_matching_original_mark_numbers() {
        use crate::annotations::{Annotations, SavedMark, SavedTool};
        use crate::export::illustration::sheets::SourceRange;
        let (device, queue) = super::super::tests::device();
        let mut input = super::super::tests::input(device, queue);
        let annotations = Annotations::restore(
            &[SavedMark {
                frame: 1,
                tool: SavedTool::Roi,
                points: vec![[0.75, 0.75], [2.75, 2.75]],
            }],
            &input.base.series,
            || false,
        )
        .unwrap();
        input.sheet = Some(Sheet::Source {
            marks: annotations.source_outlines().unwrap(),
            total: input.base.series.frame_count(),
        });
        let input = Arc::new(input);
        let choice = Choice {
            marks: true,
            masks: true,
            sheet: Some(Settings {
                columns: 2,
                range: Some(SourceRange {
                    first: 2,
                    through: 4,
                    step: 2,
                }),
            }),
        };
        let output = super::super::prepare(
            Request {
                input: input.clone(),
                choice,
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(output.size, [1032, 512]);
        let json: serde_json::Value = serde_json::from_slice(&output.manifest).unwrap();
        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["tiles"].as_array().unwrap().len(), 2);
        for (index, ordinal) in [1, 3].into_iter().enumerate() {
            assert_eq!(json["tiles"][index]["scene"]["view"]["slice"], ordinal);
            assert_eq!(json["tiles"][index]["caption"]["image"], ordinal + 1);
            assert_eq!(json["tiles"][index]["masks"]["settings"]["group"], "manual");
        }
        assert_eq!(json["tiles"][0]["marks"][0]["number"], 1);
        assert_eq!(json["tiles"][1]["marks"], serde_json::json!([]));
        assert_eq!(json["png_sha256"], hash(&output.png, &|| false).unwrap());
        assert!(!String::from_utf8_lossy(&output.manifest).contains("synthetic-worker"));
        let pixels = image::load_from_memory(&output.png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(515, 256).0, [10, 10, 10, 255]);
        assert_eq!(pixels.get_pixel(64, 64).0, [0, 0, 0, 255]);
        assert!(
            pixels.get_pixel(191, 319).0[..3]
                .iter()
                .all(|v| v.abs_diff(139) <= 1)
        );
        assert!(super::super::prepare(Request { input, choice }, &|| true).is_err());
    }
    #[cfg(feature = "gpu-tests")]
    #[test]
    fn real_gpu_mpr_sheet_keeps_three_coherent_planes_and_refuses_a_mixed_point() {
        use crate::annotations::Annotations;
        let (device, queue) = super::super::tests::device();
        let mut input = super::super::tests::input(device, queue);
        let volume = input.masks.as_ref().unwrap().source();
        let point = volume.center();
        let mut panes = Vec::new();
        for plane in [
            orv_core::Plane::Axial,
            orv_core::Plane::Coronal,
            orv_core::Plane::Sagittal,
        ] {
            let image = volume.reformat(plane, point, 64, || false).unwrap();
            let view = orv_core::ViewState {
                plane,
                slice: 0,
                window: input.base.scene.view.window,
                ..Default::default()
            };
            let outlines = Annotations::for_reformats()
                .outlines(&image, &view)
                .unwrap();
            let mut base = input.base.clone();
            base.scene.view = view;
            base.scene.crosshair_mm = Some(point);
            base.scene.reformat_side = 64;
            base.scene.output_size = [64, 64];
            base.pixels = Pixels::Reformat(image, volume.clone());
            panes.push(FrozenInput {
                base,
                outlines,
                masks: input.masks.clone(),
                mask_note: None,
                sheet: None,
            });
        }
        input.base = panes[0].base.clone();
        input.outlines = Annotations::for_reformats()
            .outlines(input.image().unwrap(), &input.base.scene.view)
            .unwrap();
        input.sheet = Some(Sheet::Mpr { panes });
        let choice = Choice {
            marks: false,
            masks: true,
            sheet: Some(Settings {
                columns: 2,
                range: None,
            }),
        };
        let input = Arc::new(input);
        let artifact = super::super::prepare(
            Request {
                input: input.clone(),
                choice,
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(artifact.size, [136, 136]);
        let metadata: serde_json::Value = serde_json::from_slice(&artifact.manifest).unwrap();
        assert_eq!(metadata["tiles"].as_array().unwrap().len(), 3);
        for (index, (plane, caption)) in [
            ("Axial", "axial"),
            ("Coronal", "coronal"),
            ("Sagittal", "sagittal"),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(metadata["tiles"][index]["scene"]["view"]["plane"], plane);
            assert_eq!(
                metadata["tiles"][index]["scene"]["crosshair_mm"],
                serde_json::to_value(point).unwrap()
            );
            assert_eq!(metadata["tiles"][index]["caption"]["kind"], caption);
            assert_eq!(
                metadata["tiles"][index]["scene"]["source_sha256"],
                artifact.source_sha256
            );
        }
        let pixels = image::load_from_memory(&artifact.png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(65, 32).0, [10, 10, 10, 255]);
        assert_eq!(pixels.get_pixel(100, 100).0, [10, 10, 10, 255]);
        assert!(!String::from_utf8_lossy(&artifact.manifest).contains("synthetic-worker"));
        drop(artifact);
        let mut input = Arc::try_unwrap(input).ok().unwrap();
        if let Some(Sheet::Mpr { panes }) = &mut input.sheet {
            panes[1].base.scene.crosshair_mm = Some([1., 2., 3.]);
        }
        let error = super::super::prepare(
            Request {
                input: Arc::new(input),
                choice,
            },
            &|| false,
        )
        .err()
        .unwrap();
        assert!(error.contains("coherent"));
    }
}
