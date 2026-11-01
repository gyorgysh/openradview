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
            egui::CollapsingHeader::new("Offline controls guide").show(ui, crate::guide::show);
            ui.label("Shortcuts in the top bar offers searchable actions and saved single-letter bindings. Tool keys are suppressed while a control or text field has focus or a menu/dialog is open. Click the image to leave a field. Tab and Shift+Tab move between controls; Enter or Space activates buttons.");
            egui::CollapsingHeader::new("Move, zoom and browse images")
                .default_open(true).show(ui, |ui| {
                    ui.label(format!("{} is the starting tool: drag or scroll to move the image. Hold Space while dragging to move temporarily from any 2D tool. Right or middle drag also moves.", crate::actions::label(ctx, crate::actions::Action::Pan)));
                    ui.label(format!("{}: drag up/down or scroll to zoom. Pinch or Cmd/Ctrl-scroll zooms in every tool and never also changes the image number.", crate::actions::label(ctx, crate::actions::Action::Zoom)));
                    ui.label(format!("{}: scroll or drag to browse slices. The image slider and ↑↓ keys work too. Crosshair mode in linked MPR also scrolls through planes.", crate::actions::label(ctx, crate::actions::Action::Browse)));
                    ui.label(format!("{}: drag to adjust brightness/contrast (window/level). Presets are starting points. Reset display restores the original image window and removes inversion; Fit centres the image and resets zoom separately.", crate::actions::label(ctx, crate::actions::Action::Contrast)));
                });
            egui::CollapsingHeader::new("Play images and save a picture").show(ui, |ui| {
                ui.label("Play browses the current 2D series at your requested speed (1–30 fps). Slower decoding reduces playback speed; images are not skipped. This does not reproduce acquisition timing. Playback stops at the end unless Loop is checked. Pause or Escape stops it; navigation, other views, dialogs and leaving the app also pause.");
                ui.label("Study → Export picture… captures the completed current pane: a PNG and reproducible view.json, up to 1024 pixels per side (3D up to 512). It preserves the displayed composition, without app text, crosshairs, measurements or labels. Checking a large source can take time; Cancel stops pending work. Original files are never modified or overwritten.");
                ui.label("Study → Export illustration… captures one source image, thin MPR/oblique plane or active comparison pane. Added layers start unchecked. Select completed measurement outlines and the shown applied mask group, then Prepare preview. Changing layers clears the preview and Save. Save reviewed illustration writes the exact reviewed PNG with illustration.json, after another source check. Masks are unavailable during an unapplied region preview; slabs and 3D are not supported. Crosshairs, scales, results, editing handles and app controls are excluded.");
                ui.label("Study → Export image range… selects up to 16 source images with First image, Through image and Image step. Review the complete Included images list; the through number is an upper bound and may not be sampled. Contact tiles start at 512 pixels square. Export MPR grid… captures all three completed thin planes at one point. Choose columns, added layers and Prepare preview. Sheets fit within 2048 pixels per side, include generic image/plane captions and save the exact reviewed PNG with version 2 illustration settings. Changing the range or columns clears preview and Save.");
                ui.label("The PNG may contain identifying text already in the pixels. The settings contain geometry and source fingerprints, without paths or patient tags. Review both files before sharing. No export is saved or shared automatically.");
            });
            egui::CollapsingHeader::new("Measurements and regions").show(ui, |ui| {
                ui.label("Measurements… also accepts typed Point column/row coordinates and Add point to measurement. Measure chooses Distance, Angle, Rectangle ROI, Ellipse ROI or Polygon ROI. Rectangle and ellipse use two opposite corners. Click polygon vertices, then Enter or Measurements… → Finish polygon. Backspace removes the last draft point; Escape cancels. Changing image, plane, mode or tool cancels the draft.");
                ui.label("ROI statistics count valid pixel centres inside the boundary. Sampled area counts valid cells; shape area describes the continuous outline, so the two can differ. Physical values need valid geometry. RGB statistics retain their declared code domain. Slab projections cannot be measured; reformats must finish updating first.");
                ui.label("Measure → Edit measurements selects or drags a vertex. Measurements… also offers a vertex selector and 1-based source column/row with Apply vertex coordinates. The original stays intact until the change validates; Escape, navigation override or a changed context cancels it. Undo/redo keeps up to 16 geometry edits in memory. Deleting or clearing marks clears that edit history. Apply or cancel coordinate changes before saving.");
                ui.label("Measurements… selects, deletes or clears current-plane marks. Each collection holds 128 marks and 4096 points, with 3–256 vertices per simple polygon and no holes or crossing edges. Marks stay in memory; an explicit comparison save can restore editable source-image marks. New shapes use comparison version 2; earlier files remain readable. Image-only exports and agent snapshots omit marks.");
            });
            egui::CollapsingHeader::new("Image information and series filters").show(ui, |ui| {
                ui.label("Image info… shows one 2D image's dimensions, reference, spacing and pixel distribution. Close it to continue navigating. Histograms count every valid pixel and ignore display window, zoom and inversion. RGB counts preserve code precision before ICC conversion. Log count scale changes the graph only.");
                ui.label("Move the pointer over an image to inspect its nearest pixel value and column/row, numbered from one. MPR and oblique values are resampled. Edge letters and an image-space scale need valid geometry; no orientation is guessed. The scale follows the image's declared spacing, not monitor calibration or guaranteed object size.");
                ui.label("Filter series by modality, series number or time/component label. Filtering narrows the list; your current series stays open until you select another.");
                ui.label("For NIfTI, Acquisition axes selects a time point, component, higher-axis index or complex view within the same imported input. Indices start at one. Known time coordinates include the source offset and step, converted to seconds; frequency coordinates remain Hz, ppm or rad/s. Unknown or invalid timing still permits index selection. Changing acquisition uses normal series selection: temporary marks clear, reconstructed views close, labels stay with their original subseries and existing sharing approval ends. Comparison protects unsaved marks before replacement. Playback remains paced image browsing.");
            });
            egui::CollapsingHeader::new("Supported files and limits").show(ui, |ui| {
                ui.label("With Open files, select DICOMDIR alone, or a ZIP containing one media index. This read-only subset opens its referenced images and reports missing files or conflicting image identities. Study numbers are generated for this opening. Open folder scans supported files directly, independently of an incomplete or unusable index. For a broken index in a ZIP, unpack it with your archive tool first; this app does not extract or repair media.");
                ui.label("Open DICOM files, folders or ZIP/ZIP64 archives. Grayscale supports native, JPEG, RLE, JPEG-LS, JPEG 2000 and deflated datasets. Color DICOM supports unsigned 8/16-bit RGB/YBR_FULL, 8-bit native packed YBR422 and direct/segmented RGB palettes. Alpha/enhanced palettes are not supported. Color supports 2D, MPR, oblique and 3D when geometry permits. Region statistics report RGB code values before ICC/display conversion. Scalar window/level does not apply.");
                ui.label("NIfTI-1/2 .nii/.nii.gz and paired .hdr/.img files also open, including gzip and same-directory pairs in ZIPs. Time points and vector/tensor components appear as separate labelled series. Complex64/128 offers magnitude, phase in radians, real and imaginary views. RGB/RGBA and float128 NIfTI are not supported. Known sform/qform geometry is converted from RAS to LPS millimetres; template coordinates are not presumed to be scanner coordinates. Unknown or invalid geometry stays unlabeled in 2D.");
                ui.label("Decoded slice caches and prepared CPU volumes use shared memory allowances based on total and available RAM, up to 8 GiB each. MPR/oblique/3D need consistent geometry, compatible pixels and enough current memory; GPU admission has separate device limits. Color code buffers count toward memory use. Compressed imports stream with progress and Cancel; no files are extracted. Backward or distant seeks in gzip/deflated data can take longer. Embedded RGB input ICC profiles convert to sRGB; without a profile, sRGB is assumed. The monitor is not calibrated by this app. RGB codes retain 8/16-bit precision; scalar values use f32. These are research display limits, not clinical validation.");
            });
            egui::CollapsingHeader::new("Local labels").show(ui, |ui| {
                ui.label("2D image → Labels… creates a local label volume. Enable Brush, close the panel and draw; Escape stops painting. Erase, opacity, two-step undo and scalar threshold fill are in the panel. Labels have no automatic anatomical meaning.");
                ui.label("Labels also appear on matching thin MPR and oblique planes. Open Labels… in those views to hide individual IDs, choose fill/outline/hatching, or inspect source voxel counts and label cell volume. Missing source samples stay separate. Reformatting and zoom do not change counts. Painting remains on 2D source images; slabs and 3D do not show labels.");
                ui.label("Load a matching NIfTI integer map (0–255), or export labels as NIfTI-2 to a new filename. Dimensions, affine and spatial reference must match; check correspondence yourself. No registration, DICOM SEG export, model inference or label overlays in agent snapshots. Labels stay in memory until export; save before closing or replacing the study.");
                ui.label("Connected regions: choose a scalar interval, source-image (4 neighbors) or volume (6 neighbors), and a source seed. Pick it on the image or enter a 1-based column/row. Preview the proposed map, then Apply or Discard. Keep seeded component and Remove small islands clean up one label ID; the cutoff removes strictly smaller components. Missing samples and other IDs are barriers. Cleanup preserves labels on missing samples. Changing inputs or view mode discards a preview; counts/exports use applied labels. No model or anatomical recognition is used.");
                ui.label("Supplied masks: select the loaded DICOM source series, open Labels… → Supplied masks, then Load matching DICOM masks…. The read-only subset accepts native little-endian BINARY SEG with matching source image/frame references and plane geometry. It preserves separate overlapping segments and sparse supplied-plane coverage.");
                ui.label("Numeric ID visibility, fill/outline/hatching and overlap crosshatching work on source and thin MPR/oblique planes. Counts include stored positives on missing samples; omitted planes are unknown, not supplied empty masks. Segment volumes overlap, so do not add them.");
                ui.label("Only the selected mask group is displayed. Manual labels keep their separate edits and undo. Closing the source releases both groups; slabs, 3D and agent snapshots exclude masks.");
            });
            egui::CollapsingHeader::new("Local comparison controls").show(ui, |ui| {
                ui.label("Use Four panes for a two-by-two grid. Pane buttons select the source controlled by the sidebar, Display and tools, keyboard shortcuts and single-picture export. Click, drag or scroll an image to activate its pane; hovering alone keeps the current selection. Display and tools can stay closed while using shortcuts.");
                ui.label("Links and reference chooses one pair of panes. The remaining panes stay independent. Changing the pair turns slice and display links off; choose the needed links again. Select one of the chosen panes to change its links.");
                ui.label("Extra panes stay in memory. Two panes asks before discarding panes 3 and 4, including their views and measurements. Save any needed pictures or illustrations first. Return to two panes to open another study through Open in pane 2 or save/open a comparison file. Existing comparison files still contain two panes.");
                ui.label("In local comparison, Reference lines starts off. Enable it to see dashed intersections of the other pane's declared thin plane within both image footprints. Both images and display settings must finish updating first. Parallel/coplanar planes and nonoverlapping footprints have no unique visible line. Unknown or different spatial references are refused; alignment is not guessed and no registration is performed.");
                ui.label("Lines follow each pane's rotation, flips, zoom and pan. Status explains an unavailable or offscreen intersection. Reference lines are local display aids; picture/illustration exports and agent snapshots omit them.");
                ui.label("Link physical pan follows the active image's physical centre. Link physical zoom matches vertical field of view in millimetres. These separate controls need parallel thin planes in the same known spatial reference; they preserve the other pane's rotation and flips. Zoom alone preserves that pane's physical centre. No registration is performed.");
                ui.label("Link scalar window follows the selected window/level choice only when scalar representations, mappings and known units match. Inversion stays independent. Physical and window links can be refused separately; status explains each result.");
                ui.label("Each user change is forwarded once. A latest pending action waits for both requested images, then cancels if you change pane ownership, the target view, sources, link choices or viewport size. Replacing a source or restoring a comparison turns display links off. Choose the links again when needed. A changed canvas size starts a fresh physical-link update for the current layout.");
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
            crate::dialogs::reveal_focus(ui);
        });
    licenses
}
