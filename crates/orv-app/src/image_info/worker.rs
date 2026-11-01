// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{
    inspection::{Histogram, SeriesInfo},
    study::{Frame, Series},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, sync_channel},
};

pub(super) struct Task {
    pub result: Receiver<Result<Inspection, &'static str>>,
    cancel: Arc<AtomicBool>,
}
pub(super) struct Inspection {
    pub histogram: Histogram,
    pub series: SeriesInfo,
}
impl Task {
    pub fn start(image: Arc<Frame>, series: Arc<Series>, ctx: egui::Context) -> Self {
        let (send, result) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let failed = send.clone();
        let repaint = ctx.clone();
        if std::thread::Builder::new()
            .name("orv-image-information".into())
            .spawn(move || {
                let cancelled = || worker.load(Ordering::Acquire);
                let result =
                    orv_core::inspection::histogram(&image, 256, cancelled).and_then(|histogram| {
                        orv_core::inspection::series_info(&series, cancelled)
                            .map(|series| Inspection { histogram, series })
                            .map_err(|_| "Series inspection cancelled or unavailable")
                    });
                if !worker.load(Ordering::Acquire) {
                    let _ = send.send(result);
                    ctx.request_repaint();
                }
            })
            .is_err()
        {
            let _ = failed.send(Err("Could not start the image information worker"));
            repaint.request_repaint();
        }
        Self { result, cancel }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
