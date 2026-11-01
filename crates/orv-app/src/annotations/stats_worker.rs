// SPDX-License-Identifier: GPL-3.0-or-later
//! Coalescing image-only statistics, bound to mark ID and geometry revision.
use super::*;
use crate::jobs::Worker;
struct Input {
    image: Arc<Frame>,
    points: Vec<[f64; 2]>,
    tool: Tool,
}
#[derive(Default)]
pub(super) struct Calculations {
    worker: Option<Worker<Input, String>>,
    pending: Option<(u64, u64, u64)>,
}
impl Calculations {
    pub fn start(
        &mut self,
        id: u64,
        revision: u64,
        image: Arc<Frame>,
        points: Vec<[f64; 2]>,
        tool: Tool,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        self.clear();
        if self.worker.is_none() {
            self.worker = Some(Worker::start(
                ctx.clone(),
                "orv-roi-statistics",
                |input: Input, cancel| report(&input.image, &input.points, input.tool, cancel),
            )?);
        }
        if let Some(worker) = &self.worker {
            self.pending = Some((
                worker.submit(Input {
                    image,
                    points,
                    tool,
                }),
                id,
                revision,
            ));
        }
        Ok(())
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn clear(&mut self) {
        self.pending = None;
        if let Some(worker) = &self.worker {
            worker.clear();
        }
    }
    pub fn cancel_for(&mut self, id: u64) {
        if self.pending.is_some_and(|(_, mark, _)| mark == id) {
            self.clear();
        }
    }
    pub fn poll(&mut self) -> Option<(u64, u64, Result<String, String>)> {
        let (generation, result) = self.worker.as_ref()?.take()?;
        let (expected, id, revision) = self.pending?;
        if generation != expected {
            return None;
        }
        self.pending = None;
        Some((id, revision, result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn wait(annotations: &mut Annotations) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while annotations.calculation.is_busy() {
            annotations.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn late_statistics_update_their_original_mark_and_deleting_cancels_that_work() {
        let image = orv_core::study::demo_study().series[0].frames[0].clone();
        let ctx = egui::Context::default();
        let mut annotations = Annotations::default();
        annotations.bind_image(&image, &ViewState::default());
        let key = annotations.image.clone().unwrap();
        annotations.marks = vec![
            Mark {
                id: 0,
                revision: 3,
                image: key.clone(),
                points: vec![[0., 0.], [3., 4.]],
                tool: Tool::Roi,
                label: "calculating".into(),
            },
            Mark {
                id: 1,
                revision: 4,
                image: key,
                points: vec![[0., 0.], [3., 4.]],
                tool: Tool::Distance,
                label: "Distance · 5.00 mm".into(),
            },
        ];
        annotations
            .calculation
            .start(
                0,
                3,
                image.clone(),
                vec![[0., 0.], [3., 4.]],
                Tool::Roi,
                &ctx,
            )
            .unwrap();
        annotations.bind_image(
            &image,
            &ViewState {
                slice: 1,
                ..Default::default()
            },
        );
        wait(&mut annotations);
        assert!(annotations.marks[0].label.starts_with("ROI · n=20"));
        assert_eq!(annotations.marks[1].label, "Distance · 5.00 mm");
        assert_eq!(annotations.visible_marks().count(), 0);
        annotations.bind_image(&image, &ViewState::default());
        annotations.selected = Some(0);
        annotations
            .calculation
            .start(0, 3, image, vec![[0., 0.], [3., 4.]], Tool::Roi, &ctx)
            .unwrap();
        annotations.delete_selected();
        assert!(!annotations.calculation.is_busy());
        assert_eq!(annotations.marks.len(), 1);
        assert_eq!(annotations.marks[0].id, 1);
    }
    #[test]
    fn worker_reports_color_codes_and_clearing_discards_late_results() {
        let mut image = (*orv_core::study::demo_study().series[0].frames[0]).clone();
        image.width = 2;
        image.height = 2;
        image.pixels = Arc::from([]);
        image.rgba = Some(Arc::from([
            0, 10, 20, 255, 100, 110, 120, 255, 0, 10, 20, 255, 100, 110, 120, 255,
        ]));
        let image = Arc::new(image);
        let ctx = egui::Context::default();
        let mut calculations = Calculations::default();
        calculations
            .start(
                0,
                7,
                image.clone(),
                vec![[0., 0.], [1., 1.]],
                Tool::Roi,
                &ctx,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let label = loop {
            if let Some((id, revision, result)) = calculations.poll() {
                assert_eq!((id, revision), (0, 7));
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert!(label.contains("sRGB presentation codes"));
        for channel in ["R · mean 50.00", "G · mean 60.00", "B · mean 70.00"] {
            assert!(label.contains(channel));
        }
        let mut annotations = Annotations {
            calculation: calculations,
            marks: vec![Mark {
                id: 0,
                revision: 8,
                image: ImageKey {
                    slab: None,
                    size: [2, 2],
                    geometry: image.geometry.clone(),
                    frame: Some(0),
                },
                points: vec![[0., 0.], [1., 1.]],
                tool: Tool::Roi,
                label: "calculating".into(),
            }],
            ..Default::default()
        };
        annotations
            .calculation
            .start(0, 8, image, vec![[0., 0.], [1., 1.]], Tool::Roi, &ctx)
            .unwrap();
        annotations.clear();
        annotations.poll();
        assert!(annotations.marks.is_empty());
        assert!(!annotations.calculation.is_busy());
    }
    #[test]
    fn changed_geometry_revision_rejects_a_completed_result_for_the_same_mark_id() {
        let image = orv_core::study::demo_study().series[0].frames[0].clone();
        let ctx = egui::Context::default();
        let mut annotations = Annotations::default();
        annotations.bind_image(&image, &ViewState::default());
        annotations.marks.push(Mark {
            id: 0,
            revision: 11,
            image: annotations.image.clone().unwrap(),
            points: vec![[1., 1.], [2., 2.]],
            tool: Tool::Roi,
            label: "New geometry pending".into(),
        });
        annotations
            .calculation
            .start(0, 10, image, vec![[0., 0.], [3., 4.]], Tool::Roi, &ctx)
            .unwrap();
        wait(&mut annotations);
        assert_eq!(annotations.marks[0].label, "New geometry pending");
        assert_eq!(annotations.marks[0].points, [[1., 1.], [2., 2.]]);
    }
}
