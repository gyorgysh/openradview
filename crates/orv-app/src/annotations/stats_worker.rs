// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable image-only ROI work; no file I/O, retained study or UI-thread scans.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, sync_channel},
};
pub(super) struct Calculation {
    pub result: Receiver<Result<String, String>>,
    cancel: Arc<AtomicBool>,
}
impl Calculation {
    pub fn start(image: Arc<Frame>, points: Vec<[f64; 2]>, ctx: egui::Context) -> Self {
        let (tx, result) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let failed = tx.clone();
        let repaint = ctx.clone();
        if std::thread::Builder::new()
            .name("orv-roi-statistics".into())
            .spawn(move || {
                let result = report(&image, &points, Tool::Roi, || {
                    worker.load(Ordering::Acquire)
                });
                if !worker.load(Ordering::Acquire) {
                    let _ = tx.send(result);
                    ctx.request_repaint();
                }
            })
            .is_err()
        {
            let _ = failed.send(Err("Could not start the rectangle statistics worker".into()));
            repaint.request_repaint();
        }
        Self { result, cancel }
    }
}
impl Drop for Calculation {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let points = vec![[0., 0.], [1., 1.]];
        let ctx = egui::Context::default();
        let task = Calculation::start(image.clone(), points.clone(), ctx.clone());
        let label = task
            .result
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(label.contains("sRGB presentation codes"));
        assert!(label.contains("R · mean 50.00"));
        assert!(label.contains("G · mean 60.00"));
        assert!(label.contains("B · mean 70.00"));
        let mut annotations = Annotations {
            complete: Some(Mark {
                points: points.clone(),
                tool: Tool::Roi,
                label: "calculating".into(),
            }),
            calculation: Some(Calculation::start(image, points, ctx)),
            ..Default::default()
        };
        annotations.clear();
        annotations.poll();
        assert!(annotations.complete.is_none());
        assert!(annotations.calculation.is_none());
    }
}
