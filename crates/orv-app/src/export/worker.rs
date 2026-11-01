// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Input, Pixels};
use eframe::egui;
use orv_core::scene::{ImageMetadata, frame_pixel_hash, series_fingerprint};
use orv_render::snapshot::{Snapshot, SnapshotRenderer};
use sha2::{Digest, Sha256};
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
        Self::start_work("orv-picture-export", ctx, move |cancelled| {
            let parent = rfd::FileDialog::new()
                .set_title("Save picture in a new OpenRadView folder")
                .pick_folder()
                .ok_or("Export cancelled. Nothing was saved.")?;
            if cancelled() {
                return Err("Export cancelled. Nothing was saved.");
            }
            let snapshot = render(input, cancelled)?;
            write_picture(&parent, &snapshot, cancelled)?;
            Ok(
                "Saved image.png and view.json in a new OpenRadView-picture folder. Review them before sharing.",
            )
        })
    }
    pub(in crate::export) fn start_work(
        name: &str,
        ctx: egui::Context,
        run: impl FnOnce(&dyn Fn() -> bool) -> Result<&'static str, &'static str> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let fallback = sender.clone();
        let repaint = ctx.clone();
        if std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = run(&|| flag.load(Ordering::Relaxed));
                let _ = sender.send(result);
                ctx.request_repaint();
            })
            .is_err()
        {
            let _ = fallback.send(Err("Could not start the export worker. Try again."));
            repaint.request_repaint();
        }
        Self { receiver, cancel }
    }
    pub(in crate::export) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn poll(&self) -> Option<Result<&'static str, &'static str>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                "The export worker stopped. Check the destination for incomplete files.",
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

fn new_folder(parent: &Path, kind: OutputKind) -> Result<PathBuf, &'static str> {
    for number in 1..=10_000 {
        let name = if number == 1 {
            kind.stem().into()
        } else {
            format!("{}-{number}", kind.stem())
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
            Err(_) => return Err("Cannot create the export folder. Choose a writable location."),
        }
    }
    Err("Too many export folders already exist here. Choose another location.")
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
    write_files(parent, &snapshot.png, &json, OutputKind::Picture, cancelled)
}
#[derive(Clone, Copy)]
enum OutputKind {
    Picture,
    Illustration,
}
impl OutputKind {
    fn stem(self) -> &'static str {
        match self {
            Self::Picture => "OpenRadView-picture",
            Self::Illustration => "OpenRadView-illustration",
        }
    }
    fn metadata(self) -> &'static str {
        match self {
            Self::Picture => "view.json",
            Self::Illustration => "illustration.json",
        }
    }
}
pub(in crate::export) fn write_illustration(
    parent: &Path,
    png: &[u8],
    json: &[u8],
    cancelled: impl Fn() -> bool,
) -> Result<PathBuf, &'static str> {
    write_files(parent, png, json, OutputKind::Illustration, cancelled)
}
fn write_files(
    parent: &Path,
    png: &[u8],
    json: &[u8],
    kind: OutputKind,
    cancelled: impl Fn() -> bool,
) -> Result<PathBuf, &'static str> {
    if cancelled() {
        return Err("Export cancelled. Nothing was saved.");
    }
    let path = new_folder(parent, kind)?;
    let files = [("image.png", png), (kind.metadata(), json)];
    let mut created = [false; 2];
    let result = (|| -> std::io::Result<()> {
        for (index, (name, bytes)) in files.iter().enumerate() {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path.join(name))?;
            created[index] = true;
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
        // Exclusive creation may fail because another file appeared. Only files
        // successfully created by this operation belong to its cleanup.
        let mut removed = true;
        for (index, (name, _)) in files.iter().enumerate() {
            if created[index]
                && let Err(error) = fs::remove_file(path.join(name))
                && error.kind() != std::io::ErrorKind::NotFound
            {
                removed = false;
            }
        }
        let _ = fs::remove_dir(&path);
        return Err(if removed {
            "Export did not finish; its partial files were removed."
        } else {
            "Export did not finish. Some partial files could not be removed; check the destination."
        });
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
    verify_volume_using(series, volume, cancelled, false)
}
pub(super) fn verify_volume_fresh(
    series: &orv_core::study::Series,
    volume: &orv_core::volume::Volume,
    cancelled: impl Fn() -> bool,
) -> Result<(), &'static str> {
    verify_volume_using(series, volume, cancelled, true)
}
fn verify_volume_using(
    series: &orv_core::study::Series,
    volume: &orv_core::volume::Volume,
    cancelled: impl Fn() -> bool,
    fresh: bool,
) -> Result<(), &'static str> {
    let count = volume.frames().len();
    if count != series.frame_count() || count > 131_072 {
        return Err("The picture source exceeds the bounded frame verification limit.");
    }
    let _scratch = orv_core::resources::MemoryBudget::volumes()
        .reserve(count as u64 * 192 + 1024 * 1024)
        .ok_or("Not enough memory for bounded source verification")?;
    let mut single = series.clone();
    single.source = None;
    single.frames.clear();
    let mut frozen = Vec::new();
    let mut current = Vec::new();
    frozen
        .try_reserve_exact(count)
        .map_err(|_| "Not enough memory for source verification")?;
    current
        .try_reserve_exact(count)
        .map_err(|_| "Not enough memory for source verification")?;
    for (index, retained) in volume.frames().iter().enumerate() {
        single.frames.clear();
        single.frames.push(retained.clone());
        frozen.push(scoped_frame_fingerprint(&single, &cancelled)?);
        single.frames.clear();
        single.frames.push(
            if fresh {
                series.read_fresh_frame(index, &cancelled)
            } else {
                series.read_frame(index, &cancelled)
            }
            .map_err(|_| "Source unavailable, changed or cancelled")?,
        );
        current.push(scoped_frame_fingerprint(&single, &cancelled)?);
    }
    frozen.sort_unstable();
    current.sort_unstable();
    if frozen != current {
        return Err("The displayed volume differs from the source. Reopen it before exporting.");
    }
    Ok(())
}
fn scoped_frame_fingerprint(
    series: &orv_core::study::Series,
    cancelled: &impl Fn() -> bool,
) -> Result<String, &'static str> {
    let public = series_fingerprint(series, cancelled)?;
    let mut digest = Sha256::new();
    digest.update(b"OpenRadView private plane reference v1\0");
    digest.update(public.as_bytes());
    if let Some(reference) = series
        .frames
        .first()
        .and_then(|frame| frame.geometry.as_ref())
        .and_then(|g| g.frame_of_reference.as_ref())
    {
        digest.update([1]);
        digest.update((reference.len() as u64).to_le_bytes());
        digest.update(reference.as_bytes());
    } else {
        digest.update([0]);
    }
    Ok(orv_core::scene::hex(&digest.finalize()))
}
