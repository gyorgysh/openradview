// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{inspection::Histogram, study::Frame};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, sync_channel},
};

pub(super) struct Task {
    pub result: Receiver<Result<Histogram, &'static str>>,
    cancel: Arc<AtomicBool>,
}
impl Task {
    pub fn start(image: Arc<Frame>, ctx: egui::Context) -> Self {
        let (send, result) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let failed = send.clone();
        let repaint = ctx.clone();
        if std::thread::Builder::new()
            .name("orv-image-histogram".into())
            .spawn(move || {
                let result =
                    orv_core::inspection::histogram(&image, 256, || worker.load(Ordering::Acquire));
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
