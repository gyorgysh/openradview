// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedded guidance. Opening help never opens a browser or starts a listener.
use eframe::egui::{self, RichText};

pub fn show(ctx: &egui::Context, open: &mut bool) -> bool {
    let mut licenses = false;
    crate::dialogs::window("Help", ctx)
        .open(open).default_width(610.0).default_height(640.0)
        .resizable(true).collapsible(false).vscroll(true)
        .show(ctx, |ui| {
            ui.heading("Using OpenRadView");
            ui.label("Personal education and research only. Not for diagnosis or clinical decisions.");
            licenses = ui.button("Third-party licenses…").clicked();
            egui::CollapsingHeader::new("Move, zoom and browse images")
                .default_open(true).show(ui, |ui| {
                    ui.label("Pan [P] is the starting tool: drag or scroll to move the image. Hold Space while dragging to move temporarily from any 2D tool. Right or middle drag also moves.");
                    ui.label("Zoom [Z]: drag up/down or scroll to zoom. Pinch or Cmd/Ctrl-scroll zooms in every tool and never also changes the image number.");
                    ui.label("Images [S]: scroll or drag to browse slices. The image slider and ↑↓ keys work too. Crosshair mode in linked MPR also scrolls through planes.");
                    ui.label("Contrast [W]: drag to adjust brightness/contrast (window/level). Presets are starting points. Reset display restores the original image window and removes inversion; Fit [R] centres the image and resets zoom separately.");
                });
            egui::CollapsingHeader::new("Play images and save a picture").show(ui, |ui| {
                ui.label("Play browses the current 2D series at your requested speed (1–30 fps). Slower decoding reduces playback speed; images are not skipped. This does not reproduce acquisition timing. Playback stops at the end unless Loop is checked. Pause or Escape stops it; navigation, other views, dialogs and leaving the app also pause.");
                ui.label("Study → Export picture… captures the completed current pane: a PNG and reproducible view.json, up to 1024 pixels per side (3D up to 512). It preserves the displayed composition, without app text, crosshairs, measurements or labels. Checking a large source can take time; Cancel stops pending work. Original files are never modified or overwritten.");
                ui.label("The PNG may contain identifying text already in the pixels. The settings contain geometry and source fingerprints, without paths or patient tags. Review both files before sharing. No export is saved or shared automatically.");
            });
            egui::CollapsingHeader::new("Image information and series filters").show(ui, |ui| {
                ui.label("Image info… shows one 2D image's dimensions, reference, spacing and pixel distribution. Close it to continue navigating. Histograms count every valid pixel and ignore display window, zoom and inversion. RGB counts preserve code precision before ICC conversion. Log count scale changes the graph only.");
                ui.label("Move the pointer over an image to inspect its nearest pixel value and column/row, numbered from one. MPR and oblique values are resampled. Edge letters and an image-space scale need valid geometry; no orientation is guessed. The scale follows the image's declared spacing, not monitor calibration or guaranteed object size.");
                ui.label("Filter series by modality, series number or time/component label. Filtering narrows the list; your current series stays open until you select another.");
            });
            egui::CollapsingHeader::new("Supported files and limits").show(ui, |ui| {
                ui.label("Open DICOM files, folders or ZIP/ZIP64 archives. Grayscale supports native, JPEG, RLE, JPEG-LS, JPEG 2000 and deflated datasets. Color DICOM supports unsigned 8/16-bit RGB/YBR_FULL, 8-bit native packed YBR422 and direct/segmented RGB palettes. Alpha/enhanced palettes are not supported. Color supports 2D, MPR, oblique and 3D when geometry permits. Rectangle statistics report RGB code values before ICC/display conversion. Scalar window/level does not apply.");
                ui.label("NIfTI-1/2 .nii/.nii.gz and paired .hdr/.img files also open, including gzip and same-directory pairs in ZIPs. Time points and vector/tensor components appear as separate labelled series. Complex64/128 offers magnitude, phase in radians, real and imaginary views. RGB/RGBA and float128 NIfTI are not supported. Known sform/qform geometry is converted from RAS to LPS millimetres; template coordinates are not presumed to be scanner coordinates. Unknown or invalid geometry stays unlabeled in 2D.");
                ui.label("Multi-GB studies use a bounded 128 MiB decoded cache. MPR/oblique/3D need consistent geometry and a separate 512 MiB decoded-volume budget. Color code buffers count toward it. Compressed imports stream with progress and Cancel; no files are extracted. Backward or distant seeks in gzip/deflated data can take longer. Embedded RGB input ICC profiles convert to sRGB; without a profile, sRGB is assumed. The monitor is not calibrated by this app. RGB codes retain 8/16-bit precision; scalar values use f32. These are research display limits, not clinical validation.");
            });
            egui::CollapsingHeader::new("Local labels").show(ui, |ui| {
                ui.label("2D image → Labels… creates a local label volume. Enable Brush, close the panel and draw; Escape stops painting. Erase, opacity, two-step undo and scalar threshold fill are in the panel. Labels have no automatic anatomical meaning.");
                ui.label("Load a matching NIfTI integer map (0–255), or export labels as NIfTI-2 to a new filename. Dimensions, affine and spatial reference must match; check correspondence yourself. No registration, DICOM SEG, model inference or label overlays in agent snapshots. Labels stay in memory until export; save before closing or replacing the study.");
            });
            egui::CollapsingHeader::new("What are MPR and 3D / MIP?").show(ui, |ui| {
                ui.label("Linked MPR shows axial, coronal and sagittal cuts through the same stack after geometry checks. Crosshair selects one shared physical point. The previous complete image stays visible while a new set is calculated.");
                ui.label("3D casts rays through the actual intensity samples and blends them using opacity settings. MIP displays the maximum intensity along each ray. These established techniques do not infer missing anatomy or segment an organ. Color MIP takes separate RGB channel maxima, which may come from different depths. Color opacity uses linear-light brightness (0–255).");
                ui.label("They need a spatial stack such as suitable CT/MRI series; a single X-ray cannot supply a 3D volume. Presets and sampling change the appearance. Keep source slices available for comparison; projections are experimental and can hide information.");
                ui.label("Demos contains public, de-identified CT, MRI and X-ray examples included with the app. Open Demos → Source credits for the creators, licenses and changes. A collection label does not establish a diagnosis for the selected image.");
            });
            egui::CollapsingHeader::new("Connect a local AI agent")
                .show(ui, |ui| {
                    if cfg!(feature = "mcp") {
                        ui.label(RichText::new("Agent connections are included in this edition. Sharing is off until you approve it.").strong());
                    } else {
                        ui.label(RichText::new("This development build omits agent support. Packaged builds can include it.").strong());
                        ui.label("Build with the mcp feature to include the connector. Installing it alone does not share anything.");
                    }
                    ui.label("1. Use an MCP-capable client running on this computer and your own account. A cloud-only client cannot reach this computer's loopback address. The viewer does not sign you into or launch an agent.");
                    ui.label("2. Open Agents and copy the setup prompt or your client's MCP configuration. Add it to your private local agent settings, then refresh its MCP connection. The reusable setup contains no session token.");
                    ui.label("3. Open one series. Ask the agent to call request_access. Review the centered dialog in this app, customize Advanced permissions if needed, then Allow for this session. Closing it denies access.");
                    ui.label("4. Ask the agent to call request_access again after you respond, then inspect open_study and render_snapshot. Revoke, changing/closing a study or series, or quitting ends the grant. Other series stay private.");
                    if ui.button("Copy introductory agent task").clicked() { ctx.copy_text(crate::copy::introductory_task()); }
                });
            #[cfg(feature = "updates")]
            egui::CollapsingHeader::new("Keep OpenRadView up to date").show(ui, |ui| {
                ui.label("Standalone installations enable automatic GitHub update checks by default. Turn them off on first entry or in Privacy → Updates, where you can also check manually. Debian installations keep automatic checks off; download a newer package from GitHub Releases and install it with your package manager. Checks contain no study data; downloading and installing require your action.");
                ui.label("Updates and installers are available from the project's GitHub Releases page.");
            });
            egui::CollapsingHeader::new("Exactly what can an agent see or change?").show(ui, |ui| {
                ui.label("render_snapshot returns an image-only PNG and reproducible view manifest, not a photograph of your desktop. get_tags returns a geometry allowlist, not raw patient tags. No tool opens arbitrary paths.");
                ui.label("With View control, set_view can change window/level, inversion, pan/zoom, slice, MPR point or supported oblique/volume settings inside the approved series. Window/level applies to scalar images. Supported color images keep RGB presentation; arbitrary colour LUTs are not implemented.");
                ui.label("An exact-view grant approves only reviewed snapshots. The wider selected-series option approves all its images and supported derived views. list_series exposes that one approved series, not your whole study. Changing series ends the grant.");
                ui.label("Measurements / ROI is a separate permission. A comparison gives each named agent the same frozen PNG/manifest and accepts experimental commentary; it cannot explore other views.");
                ui.label("Agent connections use 127.0.0.1. Update checks separately contact GitHub. Removing tags cannot erase identifying text burned into pixels.");
                ui.label(crate::copy::AGENT_PRIVACY);
            });
            ui.add_space(12.);
        });
    licenses
}
