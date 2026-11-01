// SPDX-License-Identifier: GPL-3.0-or-later
//! Counts are computed once per immutable map, independently of plane navigation.
use super::jobs::Worker;
use eframe::egui;
use orv_core::labels::{LabelStatistics, Labels};
use std::sync::{Arc, Weak};
#[derive(Default)]
pub(super) struct Counts {
    worker: Option<Worker<Arc<Labels>, LabelStatistics>>,
    map: Option<Weak<Labels>>,
    generation: u64,
    pub value: Option<LabelStatistics>,
    pub error: Option<String>,
}

impl Counts {
    pub fn update(&mut self, labels: &Arc<Labels>, ctx: &egui::Context) {
        let key = Arc::downgrade(labels);
        if self.map.as_ref().is_none_or(|old| !old.ptr_eq(&key)) {
            self.map = Some(key);
            self.value = None;
            self.error = None;
            if self.worker.is_none() {
                match Worker::start(
                    ctx.clone(),
                    "orv-label-counts",
                    |labels: Arc<Labels>, cancel| labels.statistics(cancel).map_err(str::to_owned),
                ) {
                    Ok(worker) => self.worker = Some(worker),
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                }
            }
            if let Some(worker) = &self.worker {
                self.generation = worker.submit(labels.clone());
            }
        }
        if let Some((generation, result)) = self.worker.as_ref().and_then(Worker::take)
            && generation == self.generation
        {
            match result {
                Ok(value) => self.value = Some(value),
                Err(error) => self.error = Some(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn cached_counts_follow_map_changes_and_release_the_old_map() {
        let ctx = egui::Context::default();
        let original = crate::labels::tests::labels();
        let mut filled = (*original).clone();
        filled.threshold(None, [20., 20.], 1, || false).unwrap();
        let first = Arc::new(filled);
        let key = Arc::downgrade(&first);
        let mut counts = Counts::default();
        let wait = |counts: &mut Counts, map: &Arc<Labels>| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                counts.update(map, &ctx);
                if counts.value.is_some() {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        wait(&mut counts, &first);
        assert_eq!(counts.value.as_ref().unwrap().by_id[1].valid_voxels, 192);
        let generation = counts.generation;
        counts.update(&first, &ctx);
        assert_eq!(counts.generation, generation);
        drop(first);
        assert!(key.upgrade().is_none());
        wait(&mut counts, &original);
        assert_eq!(counts.value.as_ref().unwrap().by_id[1].stored_voxels, 0);
        assert_eq!(counts.value.as_ref().unwrap().by_id[0].valid_voxels, 192);
        assert_ne!(counts.generation, generation);
    }
}
