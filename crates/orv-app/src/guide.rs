// SPDX-License-Identifier: GPL-3.0-or-later
//! Finite, manually advanced text. No source loading, timers or remote content.
use eframe::egui;
struct Step {
    title: &'static str,
    text: &'static str,
}
const STEPS: [Step; 6] = [
    Step {
        title: "Open and choose a series",
        text: "Use Open files (Cmd/Ctrl+O) for images, an archive or DICOMDIR. Open folder scans a directory directly. Tab reaches controls; Enter or Space activates buttons. Choose a series in the list. Import reasons explain why a file or advanced view is unavailable. Nothing in this guide opens a file.",
    },
    Step {
        title: "Browse and change the display",
        text: "The Image slider selects an image. Pan, Zoom, Images and Contrast select how dragging works. Fit centres the image; Reset display restores its original contrast and removes inversion. Numeric fields accept typed values; Enter commits, then Tab continues. Shortcuts lists the active keys and lets you change them.",
    },
    Step {
        title: "Explore linked planes",
        text: "Linked MPR needs a consistent volume. Its three planes meet at one physical point. Choose the active plane and enter the point coordinates, or select a crosshair position. Oblique changes a plane's direction. A previous complete image can remain visible while a worker prepares the next view. Geometry labels describe supplied coordinates; the app does not identify anatomy.",
    },
    Step {
        title: "Measure and edit",
        text: "Choose a tool under Measure. Measurements… offers 1-based Point column and Point row: enter a point and activate Add point to measurement. Distance, rectangle and ellipse use 2 points; angle uses endpoint, vertex, endpoint. Add polygon vertices then Finish polygon. Edit measurements provides a vertex selector, typed coordinates and undo/redo. Physical results need valid geometry; sampled and outline areas can differ.",
    },
    Step {
        title: "Review local regions",
        text: "Labels… keeps manual labels separate from supplied masks. Connected regions use a scalar interval, a source-image or volume neighborhood and a seed. Enter a seed column/row, prepare a proposal, then explicitly Apply or Discard it. No model or anatomical recognition is used. Counts describe source cells; supplied segment volumes can overlap. Save applied labels explicitly before closing.",
    },
    Step {
        title: "Review exports and sharing",
        text: "Study offers image-only pictures, selected-layer illustrations, source ranges and an MPR grid. Prepare and review the preview before saving. Changed choices invalidate it. Original files are not modified. Pixels can contain burned-in names. Local viewing needs no agent sharing; any agent access separately requires your session approval of the named recipient and exact scope. Use your own images for personal education and seek professional interpretation.",
    },
];
const TERMS: [(&str, &str); 10] = [
    (
        "Series",
        "A group of source images or one selected time/component representation.",
    ),
    (
        "Pixel / voxel",
        "A sample in an image / a cell in a volume. Missing samples are excluded where a validity mask is supplied.",
    ),
    (
        "Window / level",
        "The scalar range and centre mapped to display brightness; changing them does not change the source values.",
    ),
    (
        "MPR",
        "Multiplanar reconstruction: resampled planes through a volume with suitable geometry.",
    ),
    (
        "Oblique plane",
        "A plane tilted away from the standard linked directions.",
    ),
    (
        "Slab / MIP",
        "A projection through a thickness / the maximum intensity along that projection. Current measurement and mask tools require thin planes.",
    ),
    (
        "ROI",
        "A region of interest drawn for statistics. Its name conveys no anatomical or diagnostic meaning.",
    ),
    (
        "Label / segment",
        "A numbered region. Supplied segments can overlap; manual label cells hold one ID.",
    ),
    (
        "Spatial reference",
        "The coordinate system declared by the source. A matching reference alone does not establish alignment or registration.",
    ),
    (
        "Preview / applied",
        "A proposal awaiting your decision / the current accepted local state. Exports use the applied labels.",
    ),
];
pub(crate) fn show(ui: &mut egui::Ui) {
    let id = ui.id().with("offline-guide-step");
    let mut step = ui
        .data(|data| data.get_temp::<usize>(id))
        .unwrap_or(0)
        .min(STEPS.len() - 1);
    let title = ui.label(
        egui::RichText::new(format!(
            "Step {} of {} · {}",
            step + 1,
            STEPS.len(),
            STEPS[step].title
        ))
        .strong(),
    );
    ui.ctx().accesskit_node_builder(title.id, |node| {
        node.set_live(egui::accesskit::Live::Polite)
    });
    ui.label(STEPS[step].text);
    ui.horizontal_wrapped(|ui| {
        let previous = ui.add_enabled(step > 0, egui::Button::new("Previous guide step"));
        let next = ui.add_enabled(step + 1 < STEPS.len(), egui::Button::new("Next guide step"));
        if previous.clicked() {
            step = step.saturating_sub(1);
            if step == 0 {
                next.request_focus();
            }
        }
        if next.clicked() {
            step = (step + 1).min(STEPS.len() - 1);
            if step + 1 == STEPS.len() {
                previous.request_focus();
            }
        }
    });
    ui.data_mut(|data| data.insert_temp(id, step));
    ui.weak("Advance at your own pace. The guide does not navigate, alter or send your images.");
    egui::CollapsingHeader::new("Technical glossary").show(ui, |ui| {
        for (term, text) in TERMS {
            ui.label(egui::RichText::new(term).strong());
            ui.label(text);
        }
    });
}
