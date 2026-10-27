// SPDX-License-Identifier: GPL-3.0-or-later
//! Developer-only aggregate timings. Excluded from normal distribution builds.
//! Never retain event contents, image/study fields, paths or network information.
use eframe::egui;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    time::{Duration, Instant},
};

const MAX_SAMPLES: usize = 8192;

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
        let output = json!({
            "schema_version":1,"version":env!("CARGO_PKG_VERSION"),
            "build":"developer qa feature; excluded from distribution",
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
