// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{navigation::NavigationStack, study::Series};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, sync_channel},
};

pub(super) struct Task {
    result: Receiver<Result<NavigationStack, String>>,
    cancel: Arc<AtomicBool>,
}
impl Task {
    pub fn start(series: Arc<Series>, ctx: egui::Context) -> Self {
        let (tx, result) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let failed = tx.clone();
        let repaint = ctx.clone();
        if std::thread::Builder::new()
            .name("orv-physical-link".into())
            .spawn(move || {
            let result = NavigationStack::prepare(&series, || worker.load(Ordering::Acquire))
                .map_err(|error| {
                    use orv_core::spatial::GeometryError;
                    match error {
                        GeometryError::Missing => "Incomplete physical geometry",
                        GeometryError::Reference => "Unknown or mixed spatial reference",
                        GeometryError::Orientation => "Invalid or differing plane directions",
                        GeometryError::Spacing => "Invalid or differing pixel spacing",
                        GeometryError::Dimensions => "Inconsistent image dimensions",
                        GeometryError::Irregular => "Overlapping, gapped or irregular planes",
                        GeometryError::TooFew => "At least two spatially distinct images are required",
                        GeometryError::Limit => "Physical-navigation metadata exceeds the current shared memory allowance",
                        GeometryError::Cancelled => "Geometry checking cancelled",
                        GeometryError::Pixels => "Unsupported image representation",
                    }.to_owned()
                });
                if !worker.load(Ordering::Acquire) {
                    let _ = tx.send(result);
                    ctx.request_repaint();
                }
            })
            .is_err()
        {
            let _ = failed.send(Err("Could not start physical geometry checking".into()));
            repaint.request_repaint();
        }
        Self { result, cancel }
    }
    pub fn take(&self) -> Option<Result<NavigationStack, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("Physical geometry checking stopped".into()))
            }
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
