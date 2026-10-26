// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Input, Pixels};
use eframe::egui;
use orv_core::scene::{ImageMetadata, frame_pixel_hash, series_fingerprint};
use orv_render::snapshot::{Snapshot, SnapshotRenderer};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub(super) struct Task {
    receiver: mpsc::Receiver<Result<&'static str, &'static str>>,
    cancel: Arc<AtomicBool>,
}
impl Task {
    pub fn start(input: Input, ctx: egui::Context) -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let fallback = sender.clone();
        let repaint = ctx.clone();
        if std::thread::Builder::new().name("orv-picture-export".into()).spawn(move || {
            let result = (|| {
                let parent = rfd::FileDialog::new().set_title("Save picture in a new OpenRadView folder").pick_folder().ok_or("Export cancelled. Nothing was saved.")?;
                let cancelled = || flag.load(Ordering::Relaxed);
                if cancelled() { return Err("Export cancelled. Nothing was saved."); }
                let snapshot = render(input, cancelled)?;
                write_picture(&parent, &snapshot, cancelled)?;
                Ok("Saved image.png and view.json in a new OpenRadView-picture folder. Review them before sharing.")
            })();
            let _ = sender.send(result);
            ctx.request_repaint();
        }).is_err() {
            let _ = fallback.send(Err("Could not start the export worker. Try again."));
            repaint.request_repaint();
        }
        Self { receiver, cancel }
    }
    pub fn poll(&self) -> Option<Result<&'static str, &'static str>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                "The export worker stopped. No complete picture was saved.",
            )),
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(super) fn render(
    mut input: Input,
    cancelled: impl Fn() -> bool,
) -> Result<Snapshot, &'static str> {
    input.scene.validate()?;
    if cancelled() {
        return Err("Export cancelled. Nothing was saved.");
    }
    input.scene.source_sha256 = series_fingerprint(&input.series, &cancelled)?;
    // Bind the metadata to the actual retained pixels, rather than rendering an
    // older frame/volume under the fingerprint of a changed backing source.
    match &input.pixels {
        Pixels::Image(image) if input.scene.view.plane == orv_core::Plane::Acquired => {
            let source = input
                .series
                .read_frame(input.scene.view.slice as usize, &cancelled)
                .map_err(|_| "Source unavailable, changed or cancelled")?;
            if frame_pixel_hash(image, &cancelled)? != frame_pixel_hash(&source, &cancelled)?
                || ImageMetadata::from_frame(image) != ImageMetadata::from_frame(&source)
            {
                return Err(
                    "The displayed image differs from the source. Reopen it before exporting.",
                );
            }
        }
        Pixels::Volume(volume) | Pixels::Reformat(_, volume) => {
            verify_volume(&input.series, volume, &cancelled)?;
        }
        _ => (),
    }
    let mut renderer = SnapshotRenderer::with_device(input.device, input.queue);
    match input.pixels {
        Pixels::Image(image) | Pixels::Reformat(image, _) => {
            renderer.capture(&image, &input.scene, cancelled)
        }
        Pixels::Volume(volume) => renderer.capture_volume(volume, &input.scene, cancelled),
    }
}

fn new_folder(parent: &Path) -> Result<PathBuf, &'static str> {
    for number in 1..=10_000 {
        let name = if number == 1 {
            "OpenRadView-picture".into()
        } else {
            format!("OpenRadView-picture-{number}")
        };
        let path = parent.join(name);
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err("Cannot create the picture folder. Choose a writable location."),
        }
    }
    Err("Too many picture folders already exist here. Choose another location.")
}

pub(super) fn write_picture(
    parent: &Path,
    snapshot: &Snapshot,
    cancelled: impl Fn() -> bool,
) -> Result<PathBuf, &'static str> {
    if cancelled() {
        return Err("Export cancelled. Nothing was saved.");
    }
    let json = serde_json::to_vec_pretty(&snapshot.manifest)
        .map_err(|_| "Could not serialize view settings")?;
    let path = new_folder(parent)?;
    let result = (|| -> std::io::Result<()> {
        for (name, bytes) in [
            ("image.png", snapshot.png.as_slice()),
            ("view.json", json.as_slice()),
        ] {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path.join(name))?;
            for block in bytes.chunks(64 * 1024) {
                if cancelled() {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                file.write_all(block)?;
            }
            file.sync_all()?;
        }
        if cancelled() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        Ok(())
    })();
    if result.is_err() {
        // Delete only our known new outputs; never recursively remove a folder
        // whose contents could have changed outside this worker.
        let _ = fs::remove_file(path.join("image.png"));
        let _ = fs::remove_file(path.join("view.json"));
        let _ = fs::remove_dir(&path);
        return Err("Export did not finish; partial picture files were removed.");
    }
    Ok(path)
}

// Volume construction sorts along its reference normal. Verify frame identities
// as a multiset so valid left-handed NIfTI/source orders still export correctly.
pub(super) fn verify_volume(
    series: &orv_core::study::Series,
    volume: &orv_core::volume::Volume,
    cancelled: impl Fn() -> bool,
) -> Result<(), &'static str> {
    let count = volume.frames().len();
    if count != series.frame_count() || count > 131_072 {
        return Err("The picture source exceeds the bounded frame verification limit.");
    }
    let mut single = series.clone();
    single.source = None;
    single.frames.clear();
    let mut frozen = Vec::with_capacity(count);
    let mut current = Vec::with_capacity(count);
    for (index, retained) in volume.frames().iter().enumerate() {
        single.frames.clear();
        single.frames.push(retained.clone());
        frozen.push(series_fingerprint(&single, &cancelled)?);
        single.frames.clear();
        single.frames.push(
            series
                .read_frame(index, &cancelled)
                .map_err(|_| "Source unavailable, changed or cancelled")?,
        );
        current.push(series_fingerprint(&single, &cancelled)?);
    }
    frozen.sort_unstable();
    current.sort_unstable();
    if frozen != current {
        return Err("The displayed volume differs from the source. Reopen it before exporting.");
    }
    Ok(())
}
