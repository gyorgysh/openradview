// SPDX-License-Identifier: GPL-3.0-or-later
//! Developer-only aggregate timings. Excluded from normal distribution builds.
//! Never retain event contents, image/study fields, paths or network information.
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{self, Write},
    time::{Duration, Instant},
};

const MAX_SAMPLES: usize = 8192;
const MAX_PENDING_MPR: usize = 256;

#[derive(Default)]
pub struct MprTracking {
    pending: VecDeque<(u64, Instant)>,
    samples: Vec<MprSample>,
    requests: u64,
    evicted: u64,
    unmatched: u64,
}
struct MprSample {
    elapsed_ms: f64,
    preview: bool,
    superseded: bool,
    slab: bool,
    all_gpu: bool,
}
impl MprTracking {
    pub fn request(&mut self, generation: u64, at: Instant) {
        self.requests += 1;
        if self.pending.len() == MAX_PENDING_MPR {
            self.pending.pop_front();
            self.evicted += 1;
        }
        self.pending.push_back((generation, at));
    }
    pub fn snapshot(
        &mut self,
        generation: u64,
        current: u64,
        at: Instant,
        preview: bool,
        slab: bool,
        all_gpu: bool,
    ) {
        if let Some((_, started)) = self.pending.iter().find(|(id, _)| *id == generation) {
            if let Some(elapsed) = at.checked_duration_since(*started)
                && self.samples.len() < MAX_SAMPLES
            {
                self.samples.push(MprSample {
                    elapsed_ms: elapsed.as_secs_f64() * 1000.,
                    preview,
                    superseded: generation != current,
                    slab,
                    all_gpu,
                });
            }
        } else {
            self.unmatched += 1;
        }
        if !preview {
            self.pending.retain(|(id, _)| *id > generation);
        }
    }
    pub fn suspend(&mut self) {
        self.pending.clear();
    }
}

pub struct Recorder {
    started: Instant,
    input_started: Option<Instant>,
    first_ui_ms: Option<f64>,
    frames: u64,
    input_frames: u64,
    ui_ms: Vec<f64>,
    dispatch_ms: Vec<f64>,
    integration_ms: Vec<f64>,
    native_smoke: bool,
    smoke_started: Option<Instant>,
    quiet_ui_passes: u64,
    mpr_requests: u64,
    mpr_evicted: u64,
    mpr_unmatched: u64,
    mpr_dropped: u64,
    mpr_samples: Vec<MprSample>,
}
impl Recorder {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            input_started: None,
            first_ui_ms: None,
            frames: 0,
            input_frames: 0,
            ui_ms: Vec::new(),
            dispatch_ms: Vec::new(),
            integration_ms: Vec::new(),
            native_smoke: !cfg!(any(feature = "mcp", feature = "updates"))
                && std::env::var_os("ORV_QA_NATIVE_SMOKE").is_some_and(|value| value == "1"),
            smoke_started: None,
            quiet_ui_passes: 0,
            mpr_requests: 0,
            mpr_evicted: 0,
            mpr_unmatched: 0,
            mpr_dropped: 0,
            mpr_samples: Vec::new(),
        }
    }
    pub fn native_smoke_enabled(&self) -> bool {
        self.native_smoke
    }
    /// Explicit QA-only empty-window probe. It never opens input or changes consent.
    pub fn native_smoke_tick(&mut self, ctx: &egui::Context) {
        if !self.native_smoke {
            return;
        }
        let elapsed = self
            .smoke_started
            .get_or_insert_with(Instant::now)
            .elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.quiet_ui_passes += 1;
        }
        if elapsed >= Duration::from_secs(4) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            // Leave a timer margin instead of issuing sub-millisecond wakeups near exit.
            ctx.request_repaint_after(Duration::from_secs(4) - elapsed + Duration::from_millis(10));
        }
    }
    pub fn input(&mut self, has_events: bool) {
        self.input_started = has_events.then(Instant::now);
    }
    pub fn mpr(&mut self, tracking: &mut MprTracking) {
        self.mpr_requests += std::mem::take(&mut tracking.requests);
        self.mpr_evicted += std::mem::take(&mut tracking.evicted);
        self.mpr_unmatched += std::mem::take(&mut tracking.unmatched);
        let remaining = MAX_SAMPLES.saturating_sub(self.mpr_samples.len());
        self.mpr_dropped += tracking.samples.len().saturating_sub(remaining) as u64;
        self.mpr_samples
            .extend(tracking.samples.drain(..).take(remaining));
    }
    pub fn frame(&mut self, cpu: Duration, previous_integration_cpu_seconds: Option<f32>) {
        self.first_ui_ms
            .get_or_insert_with(|| self.started.elapsed().as_secs_f64() * 1000.);
        self.frames += 1;
        push(&mut self.ui_ms, cpu.as_secs_f64() * 1000.);
        if let Some(started) = self.input_started.take() {
            self.input_frames += 1;
            push(
                &mut self.dispatch_ms,
                started.elapsed().as_secs_f64() * 1000.,
            );
        }
        if let Some(seconds) = previous_integration_cpu_seconds {
            push(&mut self.integration_ms, f64::from(seconds) * 1000.);
        }
    }
    pub fn finish(self) -> io::Result<()> {
        let path = std::env::temp_dir().join(format!("OpenRadView-qa-{}.json", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        let mpr_samples = &self.mpr_samples;
        let mpr_groups: Vec<_> = [false, true]
            .into_iter()
            .flat_map(|slab| {
                [false, true].into_iter().flat_map(move |all_gpu| {
                    [(true, false), (true, true), (false, false), (false, true)]
                        .into_iter()
                        .map(move |(preview, superseded)| {
                            let times = mpr_samples
                                .iter()
                                .filter(|sample| {
                                    sample.slab == slab
                                        && sample.all_gpu == all_gpu
                                        && sample.preview == preview
                                        && sample.superseded == superseded
                                })
                                .map(|sample| sample.elapsed_ms)
                                .collect();
                            json!({"slab":slab,"all_three_gpu":all_gpu,"preview":preview,
                        "superseded":superseded,"request_to_ui_poll_ms":summary(times)})
                        })
                })
            })
            .collect();
        let output = json!({
            "schema_version":1,"version":env!("CARGO_PKG_VERSION"),
            "build":"developer qa feature; excluded from distribution",
            "linked_mpr_cpu_forced":option_env!("ORV_QA_MPR_CPU") == Some("1"),
            "scope":"CPU framework dispatch to UI completion; not OS input-to-photon or GPU presentation latency",
            "native_window_smoke":self.native_smoke.then(|| json!({
                "quiet_ui_passes":self.quiet_ui_passes,"quiet_after_seconds":1,"close_after_seconds":4,
                "scope":"Empty native research notice; no study opened or consent changed"
            })),
            "first_ui_cpu_complete_ms":self.first_ui_ms,
            "elapsed_seconds":self.started.elapsed().as_secs_f64(),
            "ui_passes":self.frames,"input_ui_passes":self.input_frames,
            "sample_limit_per_metric":MAX_SAMPLES,
            "ui_cpu_ms":summary(self.ui_ms),"input_dispatch_ui_cpu_ms":summary(self.dispatch_ms),
            "eframe_previous_cpu_ms":summary(self.integration_ms),
            "linked_mpr": {"requests":self.mpr_requests,"evicted_requests":self.mpr_evicted,
                "unmatched_snapshots":self.mpr_unmatched,"dropped_samples":self.mpr_dropped,"groups":mpr_groups,
                "scope":"UI request to worker snapshot consumed by the UI, before rendering/presentation; coalesced requests may have no sample"},
            "data":"timing/count aggregates only; no input contents, images, tags or paths"
        });
        serde_json::to_writer_pretty(&mut file, &output).map_err(io::Error::other)?;
        file.write_all(b"\n")
    }
}
fn push(samples: &mut Vec<f64>, value: f64) {
    if samples.len() < MAX_SAMPLES && value.is_finite() && value >= 0. {
        samples.push(value);
    }
}
fn summary(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let quantile = |fraction: f64| -> Option<f64> {
        samples
            .get(((samples.len() as f64 * fraction).ceil() as usize).saturating_sub(1))
            .copied()
    };
    json!({"samples":samples.len(),"median":quantile(0.5),"p95":quantile(0.95),"max":samples.last()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpr_timings_match_generations_and_bound_retained_requests() {
        let start = Instant::now();
        let mut tracking = MprTracking::default();
        tracking.request(1, start);
        tracking.request(2, start + Duration::from_millis(4));
        tracking.snapshot(1, 2, start + Duration::from_millis(10), true, false, true);
        tracking.snapshot(2, 2, start + Duration::from_millis(16), true, true, false);
        tracking.snapshot(2, 2, start + Duration::from_millis(150), false, true, false);
        tracking.snapshot(1, 2, start + Duration::from_millis(160), true, false, true);
        assert_eq!(tracking.unmatched, 1);
        assert!(tracking.pending.is_empty());
        assert_eq!(tracking.samples.len(), 3);
        assert!(tracking.samples[0].superseded && tracking.samples[0].all_gpu);
        assert_eq!(tracking.samples[0].elapsed_ms, 10.);
        assert!(tracking.samples[1].slab && !tracking.samples[1].superseded);
        assert_eq!(tracking.samples[1].elapsed_ms, 12.);
        assert!(!tracking.samples[2].preview);
        assert_eq!(tracking.samples[2].elapsed_ms, 146.);
        let mut recorder = Recorder::new(start);
        recorder.mpr(&mut tracking);
        assert_eq!(recorder.mpr_requests, 2);
        assert_eq!(recorder.mpr_unmatched, 1);
        assert_eq!(recorder.mpr_samples.len(), 3);
        assert!(tracking.samples.is_empty());
        for generation in 0..=MAX_PENDING_MPR as u64 {
            tracking.request(generation, start);
        }
        assert_eq!(tracking.pending.len(), MAX_PENDING_MPR);
        assert_eq!(tracking.evicted, 1);
        tracking.suspend();
        assert!(tracking.pending.is_empty());
    }
}
