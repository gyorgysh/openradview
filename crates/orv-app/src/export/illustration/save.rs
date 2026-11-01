// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::export::worker::{self, Task};

pub(super) fn start(artifact: Arc<Artifact>, ctx: egui::Context) -> Task {
    Task::start_work("orv-illustration-save", ctx, move |cancel| {
        let parent = rfd::FileDialog::new()
            .set_title("Save reviewed illustration in a new OpenRadView folder")
            .pick_folder()
            .ok_or("Export cancelled. Nothing was saved.")?;
        save(&parent, &artifact, cancel)?;
        Ok(
            "Saved the reviewed image.png and illustration.json in a new OpenRadView-illustration folder. Review both before sharing.",
        )
    })
}
fn save(
    parent: &std::path::Path,
    artifact: &Artifact,
    cancel: &dyn Fn() -> bool,
) -> Result<(), &'static str> {
    if cancel() {
        return Err("Export cancelled. Nothing was saved.");
    }
    let current = render::verify_source(&artifact.input, artifact.choice.masks, cancel)
        .map_err(|_| "Source unavailable, changed or cancelled. No illustration was saved; reopen export to prepare a new preview.")?;
    if current != artifact.source_sha256 {
        return Err("Source changed since the preview. No illustration was saved; reopen export.");
    }
    worker::write_illustration(parent, &artifact.png, &artifact.manifest, cancel)?;
    // A cancellation arriving after both files finished cannot undo that result.
    Ok(())
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests {
    use super::*;
    #[test]
    fn save_reuses_exact_reviewed_bytes_and_refuses_source_changes_or_cancel() {
        let (device, queue) = render::tests::device();
        let captured = Arc::new(render::tests::input(device.clone(), queue.clone()));
        let mut artifact = render::prepare(
            Request {
                input: captured,
                choice: Choice {
                    marks: true,
                    masks: true,
                },
            },
            &|| false,
        )
        .unwrap();
        let parent = tempfile::tempdir().unwrap();
        assert!(save(parent.path(), &artifact, &|| true).is_err());
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
        save(parent.path(), &artifact, &|| false).unwrap();
        let folder = parent.path().join("OpenRadView-illustration");
        assert_eq!(
            std::fs::read(folder.join("image.png")).unwrap(),
            artifact.png
        );
        assert_eq!(
            std::fs::read(folder.join("illustration.json")).unwrap(),
            artifact.manifest
        );
        let changed = Arc::get_mut(&mut artifact).unwrap();
        let mut captured = render::tests::input(device, queue);
        let series = Arc::make_mut(&mut captured.base.series);
        Arc::make_mut(&mut series.frames[0]).pixels = vec![999.; 16].into();
        changed.input = Arc::new(captured);
        assert!(save(parent.path(), &artifact, &|| false).is_err());
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(folder.join("image.png")).unwrap(),
            artifact.png
        );
    }
}
