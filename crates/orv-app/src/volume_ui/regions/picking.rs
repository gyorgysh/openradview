// SPDX-License-Identifier: GPL-3.0-or-later
//! A local, read-only click probe with originating-frame and live-view checks.
use super::*;
use orv_render::{
    material_volume::Lighting,
    surface_pick::{GpuSurfacePicker, PickFrame, PickState, SurfaceProbe, SurfaceProbeKind},
};

struct Job {
    engine: Arc<Mutex<Option<GpuSurfacePicker>>>,
    source: Arc<GpuVolumeSource>,
    frame: PickFrame,
    current: PickState,
    pixel: [u32; 2],
}
struct Pending {
    generation: u64,
    pixel: [u32; 2],
    rect: egui::Rect,
}
pub(super) struct Probe {
    worker: Worker<Job, SurfaceProbe>,
    engine: Arc<Mutex<Option<GpuSurfacePicker>>>,
    pending: Option<Pending>,
    notice: Option<String>,
}
impl Probe {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        Ok(Self {
            worker: Worker::start(ctx, "orv-surface-probe", run)?,
            engine: Arc::new(Mutex::new(None)),
            pending: None,
            notice: None,
        })
    }
    pub fn clear(&mut self) {
        self.worker.clear();
        self.pending = None;
        self.notice = None;
    }
    pub fn show(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        front: Option<&Completed>,
        key: &Key,
        source: &Arc<GpuVolumeSource>,
        interacting: bool,
    ) -> Option<[f64; 3]> {
        let rect = response.rect;
        if interacting {
            self.clear();
            return None;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.rect != rect)
        {
            self.clear();
        }
        let live = front.and_then(|front| current(front, key));
        let mut selected = None;
        if let Some((generation, result)) = self.worker.take()
            && let Some(pending) = self.pending.take()
            && pending.generation == generation
            && pending.rect == rect
            && let Some((_, desired)) = &live
        {
            match result {
                Ok(probe) if probe.is_current(desired, pending.pixel) => {
                    self.notice = Some(match probe.kind() {
                        SurfaceProbeKind::Hit => {
                            if let Some(hit) = probe.hit() {
                                let cell = hit.face.cell();
                                let point = source.volume().voxel_to_world(cell.map(f64::from));
                                selected = Some(point);
                                format!(
                                    "Source cell {}, {}, {} · L {:.2} P {:.2} S {:.2} mm",
                                    cell[0], cell[1], cell[2], point[0], point[1], point[2]
                                )
                            } else {
                                "No checked source cell".into()
                            }
                        }
                        SurfaceProbeKind::Empty => "No known cell face at this point".into(),
                        SurfaceProbeKind::Obscured => {
                            "Cell face obscured by foreground source".into()
                        }
                        SurfaceProbeKind::Ambiguous => {
                            "Cell-face contact is ambiguous; choose another point".into()
                        }
                    });
                }
                Err(error) if desired.scope.is_active() => self.notice = Some(error),
                _ => (),
            }
        }
        if response.hovered() && live.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if !interacting
            && response.clicked_by(egui::PointerButton::Primary)
            && !response.double_clicked()
            && !crate::viewport::navigation_override(ui, response)
            && ui.input(|input| input.modifiers == egui::Modifiers::NONE)
        {
            selected = None;
            if let Some((frame, current)) = live
                && let Some(pixel) = response
                    .interact_pointer_pos()
                    .and_then(|point| pixel_at(rect, point, current.size))
            {
                let generation = self.worker.submit(Job {
                    engine: self.engine.clone(),
                    source: source.clone(),
                    frame,
                    current,
                    pixel,
                });
                self.pending = Some(Pending {
                    generation,
                    pixel,
                    rect,
                });
                self.notice = Some("Checking the selected cell face…".into());
            } else {
                self.notice = Some("Wait for the current surface frame before picking".into());
            }
        }
        if let Some(notice) = &self.notice {
            let painter = ui.painter_at(rect);
            let text = painter.layout(
                notice.clone(),
                egui::FontId::monospace(10.),
                crate::theme::PAPER,
                (rect.width() - 12.).max(16.),
            );
            let origin = rect.left_bottom() + egui::vec2(6., -6. - text.size().y);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    origin - egui::vec2(3., 2.),
                    text.size() + egui::vec2(6., 4.),
                ),
                2.,
                egui::Color32::BLACK,
            );
            painter.galley(origin, text, crate::theme::PAPER);
        }
        selected
    }
}

fn current(front: &Completed, key: &Key) -> Option<(PickFrame, PickState)> {
    if !key.pick || !front.key.same(key) {
        return None;
    }
    let Image::Faces(image) = &*front.image else {
        return None;
    };
    let scope = front.scope.as_ref()?;
    if !scope.is_active() || *image.scene().material != key.style.material {
        return None;
    }
    let frame = PickFrame::new(image.clone(), scope.clone());
    let current = PickState {
        scope: scope.clone(),
        map: key.selection.map.clone(),
        number: key.selection.number,
        navigation: key.display.clone(),
        volume: key.state.clone(),
        material: image.scene().material.clone(),
        lighting: key.style.lighting,
        surface_lighting: if key.surface_lighting {
            Lighting::HEADLIGHT
        } else {
            Lighting::UNLIT
        },
        surface_luminance: key.luminance,
        size: if key.state.quality == Quality::Refined {
            key.refined
        } else {
            key.interactive
        },
    };
    current.matches(&frame).then_some((frame, current))
}
fn pixel_at(rect: egui::Rect, point: egui::Pos2, size: [u32; 2]) -> Option<[u32; 2]> {
    if !rect.is_positive()
        || !rect.min.is_finite()
        || !rect.max.is_finite()
        || !point.is_finite()
        || !rect.contains(point)
        || size.contains(&0)
    {
        return None;
    }
    let uv = (point - rect.min) / rect.size();
    let uv = [uv.x, uv.y];
    Some(std::array::from_fn(|axis| {
        ((uv[axis] * size[axis] as f32).floor().max(0.) as u32).min(size[axis] - 1)
    }))
}
fn run(input: Job, cancel: &dyn Fn() -> bool) -> Result<SurfaceProbe, String> {
    let cancelled = || cancel() || !input.current.scope.is_active();
    if cancelled() {
        return Err("Cancelled".into());
    }
    let mut engine = input
        .engine
        .lock()
        .map_err(|_| "Surface probe unavailable; reopen the view")?;
    if cancelled() {
        return Err("Cancelled".into());
    }
    if engine.is_none() {
        *engine = Some(GpuSurfacePicker::new(input.source).map_err(str::to_owned)?);
    }
    engine
        .as_ref()
        .ok_or("Surface probe unavailable")?
        .probe(&input.frame, &input.current, input.pixel, cancelled)
        .map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_mapping_uses_the_completed_image_size_and_rejects_outside_points() {
        let rect = egui::Rect::from_min_size(egui::pos2(731., 240.), egui::vec2(139., 123.));
        assert_eq!(pixel_at(rect, rect.min, [96, 64]), Some([0, 0]));
        assert_eq!(pixel_at(rect, rect.center(), [96, 64]), Some([48, 32]));
        assert_eq!(pixel_at(rect, rect.max, [96, 64]), Some([95, 63]));
        assert_eq!(pixel_at(rect, rect.center(), [2048, 128]), Some([1024, 64]));
        assert!(pixel_at(rect, rect.min - egui::vec2(0.1, 0.), [96, 64]).is_none());
        assert!(pixel_at(rect, egui::pos2(f32::NAN, 0.), [96, 64]).is_none());
        assert!(pixel_at(rect, rect.center(), [0, 64]).is_none());
    }

    #[cfg(feature = "gpu-tests")]
    #[test]
    fn newer_mpr_navigation_cancels_a_probe_without_resubmitting_unchanged_3d() {
        use crate::volume_ui::regions::tests::gpu;
        use orv_core::labels::Labels;
        use std::time::{Duration, Instant};
        let source = gpu::source();
        let ctx = egui::Context::default();
        let mut labels = Labels::empty(source.volume().clone()).unwrap();
        labels.threshold(None, [0., 100.], 7, || false).unwrap();
        let mut key = gpu::key(crate::labels::RegionMap::Manual(Arc::new(labels)), 7);
        key.geometry = Geometry::Faces;
        key.pick = true;
        key.state.quality = Quality::Interactive;
        let scope = PickScope::new().unwrap();
        let mut request = gpu::request(&source, key.clone());
        request.scope = Some(scope.clone());
        let front = worker::run(request, &|| false).unwrap();
        let retained = front.image.clone();
        let (frame, current) = current(&front, &key).unwrap();
        let mut probe = Probe::new(ctx.clone()).unwrap();
        let engine = probe.engine.clone();
        let weak = Arc::downgrade(&engine);
        let blocked = engine.lock().unwrap();
        let pixel = [24, 16];
        let generation = probe.worker.submit(Job {
            engine: engine.clone(),
            source: source.clone(),
            frame,
            current,
            pixel,
        });
        probe.pending = Some(Pending {
            generation,
            pixel,
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(139., 123.)),
        });
        let mut view = View::new(source.clone(), wgpu::TextureFormat::Rgba8Unorm, ctx).unwrap();
        view.front = Some(front);
        view.scope = Some(scope.clone());
        view.key = Some(key);
        view.probe = Some(probe);
        let mut regions = Regions {
            view: Some(view),
            link_epoch: Some(1),
            ..Default::default()
        };
        regions.set_link_epoch(2);
        let view = regions.view.as_ref().unwrap();
        assert_eq!(
            view.generation, 0,
            "Moving MPR must not rerender unchanged 3D"
        );
        assert!(
            scope.is_active(),
            "The unchanged image still has the same valid scene"
        );
        assert!(Arc::ptr_eq(&retained, &view.front.as_ref().unwrap().image));
        assert!(view.probe.as_ref().unwrap().pending.is_none());
        drop(blocked);
        drop(regions);
        drop(engine);
        let until = Instant::now() + Duration::from_secs(10);
        while weak.upgrade().is_some() {
            assert!(Instant::now() < until, "Cancelled probe did not retire");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!scope.is_active());
        source.wait_submitted(|| false).unwrap();
    }

    #[cfg(feature = "gpu-tests")]
    #[test]
    fn native_surface_probe_rechecks_live_geometry_view_scope_and_foreground_opacity() {
        use crate::volume_ui::regions::tests::gpu;
        use orv_core::{
            labels::Labels,
            volume_material::{MaterialPoint, VolumeMaterial},
        };
        let source = gpu::source();
        let mut labels = Labels::empty(source.volume().clone()).unwrap();
        labels.threshold(None, [0., 100.], 7, || false).unwrap();
        let labels = Arc::new(labels);
        let mut key = gpu::key(crate::labels::RegionMap::Manual(labels.clone()), 7);
        key.geometry = Geometry::Faces;
        key.surface_lighting = false;
        key.pick = true;
        let mut input = gpu::request(&source, key.clone());
        input.scope = Some(PickScope::new().unwrap());
        let early = worker::run(input, &|| false).unwrap();
        assert!(
            current(&early, &key).is_none(),
            "Preview optics must not authorize the desired refined view"
        );
        let scope = PickScope::new().unwrap();
        let mut input = gpu::request(&source, key.clone());
        input.scope = Some(scope.clone());
        input.refined = true;
        input.size = key.refined;
        let front = worker::run(input, &|| false).unwrap();
        let (frame, desired) = current(&front, &key).unwrap();
        let engine = Arc::new(Mutex::new(None));
        let job = |frame: PickFrame, current: PickState| Job {
            engine: engine.clone(),
            source: source.clone(),
            frame,
            current,
            pixel: [48, 32],
        };
        let probe = run(job(frame.clone(), desired.clone()), &|| false).unwrap();
        assert_eq!(probe.kind(), SurfaceProbeKind::Hit);
        assert_eq!(probe.hit().unwrap().face.cell()[1], 0);
        assert!(probe.is_current(&desired, [48, 32]));
        assert!(!probe.is_current(&desired, [47, 32]));
        for change in 0..8 {
            let mut changed = key.clone();
            match change {
                0 => changed.state.camera.yaw_degrees = 20.,
                1 => changed.refined[0] += 1,
                2 => changed.surface_lighting = true,
                3 => changed.pick = false,
                4 => changed.geometry = Geometry::Cells,
                5 => changed.luminance = 0.1,
                6 => {
                    changed.selection =
                        crate::labels::RegionSelection::new(changed.selection.map, 8).unwrap()
                }
                _ => {
                    let mut edited = labels.try_clone(|| false).unwrap();
                    edited.paint(1, &[[4., 4.]], 1, 0, || false).unwrap();
                    changed.selection = crate::labels::RegionSelection::new(
                        crate::labels::RegionMap::Manual(Arc::new(edited)),
                        7,
                    )
                    .unwrap();
                }
            }
            assert!(current(&front, &changed).is_none());
        }
        // An interior mask has source foreground. Zero source luminance cannot
        // prove visibility: opacity must be checked by the actual pixel optics.
        let mut inside = Labels::empty(source.volume().clone()).unwrap();
        inside.threshold(Some(1), [0., 100.], 7, || false).unwrap();
        key.selection = crate::labels::RegionSelection::new(
            crate::labels::RegionMap::Manual(Arc::new(inside)),
            7,
        )
        .unwrap();
        key.state.camera.yaw_degrees = 0.;
        key.state.camera.pitch_degrees = 90.;
        key.style.material = VolumeMaterial::new(&[0., 100.].map(|value| MaterialPoint {
            value,
            luminance: 0.,
            extinction_per_mm: 10.,
        }))
        .unwrap();
        let mut input = gpu::request(&source, key.clone());
        input.refined = true;
        input.size = key.refined;
        input.scope = Some(scope.clone());
        let obscured = worker::run(input, &|| false).unwrap();
        let (obscured_frame, obscured_state) = current(&obscured, &key).unwrap();
        let hidden = run(job(obscured_frame, obscured_state), &|| false).unwrap();
        assert_eq!(hidden.kind(), SurfaceProbeKind::Obscured);
        assert!(hidden.hit().is_none());
        scope.revoke();
        assert!(current(&front, &front.key).is_none());
        assert!(run(job(frame, desired), &|| false).is_err());
        source.wait_submitted(|| false).unwrap();
    }
}
