// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded non-study settings; all filesystem work stays on a sleeping worker.
use crate::actions::{self, Bindings};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema_version: u8,
    bindings: Bindings,
}
enum Request {
    Load,
    Save(Bindings),
}
fn location() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let folder = PathBuf::from(std::env::var_os("HOME").ok_or("Settings folder unavailable")?)
        .join("Library/Application Support/OpenRadView");
    #[cfg(target_os = "windows")]
    let folder =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Settings folder unavailable")?)
            .join("OpenRadView");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let folder = match std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(path) => path.join("openradview"),
        None => PathBuf::from(std::env::var_os("HOME").ok_or("Settings folder unavailable")?)
            .join(".config/openradview"),
    };
    if !folder.is_absolute() {
        return Err("Settings folder unavailable".into());
    }
    Ok(folder.join("controls.json"))
}
fn load(path: &Path) -> Result<Bindings, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Bindings::default());
        }
        Err(_) => return Err("Could not read shortcut settings; defaults are in use".into()),
    };
    if !metadata.is_file() || metadata.len() > 8192 {
        return Err("Shortcut settings are not a small regular file; defaults are in use".into());
    }
    let mut bytes = Vec::with_capacity(8193);
    fs::File::open(path)
        .and_then(|file| file.take(8193).read_to_end(&mut bytes))
        .map_err(|_| "Could not read shortcut settings; defaults are in use")?;
    if bytes.len() > 8192 {
        return Err("Shortcut settings exceed the size limit".into());
    }
    let file: File = serde_json::from_slice(&bytes)
        .map_err(|_| "Shortcut settings are invalid; defaults are in use")?;
    if file.schema_version != 1 {
        return Err("Shortcut settings have an unsupported version".into());
    }
    file.bindings.validate()?;
    Ok(file.bindings)
}
fn save(path: &Path, bindings: Bindings, cancel: &dyn Fn() -> bool) -> Result<Bindings, String> {
    bindings.validate()?;
    let parent = path.parent().ok_or("Settings folder unavailable")?;
    fs::create_dir_all(parent).map_err(|_| "Could not create the settings folder")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "Could not create shortcut settings")?;
    let bytes = serde_json::to_vec(&File {
        schema_version: 1,
        bindings,
    })
    .map_err(|_| "Could not encode shortcut settings")?;
    file.write_all(&bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| "Could not finish shortcut settings")?;
    if cancel() {
        return Err("Shortcut save cancelled".into());
    }
    file.persist(path)
        .map_err(|_| "Could not replace shortcut settings")?;
    Ok(bindings)
}
fn run(request: Request, cancel: &dyn Fn() -> bool) -> Result<Bindings, String> {
    let path = location()?;
    match request {
        Request::Load => load(&path),
        Request::Save(bindings) => save(&path, bindings, cancel),
    }
}

pub(crate) struct Preferences {
    pub open: bool,
    active: Bindings,
    draft: Bindings,
    worker: Option<crate::jobs::Worker<Request, Bindings>>,
    pending: Option<u64>,
    loaded: bool,
    started: bool,
    query: String,
    message: String,
    focus_search: bool,
    return_focus: Option<egui::Id>,
}
impl Preferences {
    pub fn new() -> Self {
        Self {
            open: false,
            active: Bindings::default(),
            draft: Bindings::default(),
            worker: None,
            pending: None,
            loaded: false,
            started: false,
            query: String::new(),
            message: "Loading shortcuts…".into(),
            focus_search: false,
            return_focus: None,
        }
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            match crate::jobs::Worker::start(ctx.clone(), "local-control-settings", run) {
                Ok(worker) => {
                    self.pending = Some(worker.submit(Request::Load));
                    self.worker = Some(worker);
                }
                Err(error) => self.message = error,
            }
        }
        if let Some((generation, result)) = self.worker.as_ref().and_then(|worker| worker.take())
            && self.pending == Some(generation)
        {
            self.pending = None;
            let loading = !self.loaded;
            self.loaded = true;
            match result {
                Ok(bindings) => {
                    self.active = bindings;
                    self.draft = bindings;
                    self.message = if loading {
                        "Shortcuts ready"
                    } else {
                        "Shortcuts saved on this device"
                    }
                    .into();
                }
                Err(error) => self.message = error,
            }
        }
        actions::install(ctx, self.active);
    }
    pub fn open(&mut self, ctx: &egui::Context) {
        self.return_focus = ctx.memory(|memory| memory.focused());
        self.focus_search = true;
        self.open = true;
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        let was_open = self.open;
        let mut open = self.open;
        let mut close = false;
        let mut focus_close = false;
        crate::dialogs::window("Keyboard shortcuts", ctx).open(&mut open)
            .default_width(550.).default_height(610.).show(ctx, |ui| {
                ui.label("Search local actions and type a single A–Z shortcut. Select the current letter to replace it. Clear leaves the action available through its controls.");
                let query_label = ui.label("Find an action");
                let response = ui.add(egui::TextEdit::singleline(&mut self.query)
                    .id(egui::Id::new("shortcut-query")).char_limit(64)
                    .desired_width(f32::INFINITY)).labelled_by(query_label.id);
                if self.focus_search { response.request_focus(); self.focus_search = false; }
                ui.weak("Tool keys work when no control or text field has keyboard focus. Click the image to leave a field. Modifiers and open menus suppress tool shortcuts.");
                let query = self.query.to_ascii_lowercase();
                ui.add_enabled_ui(self.pending.is_none() && self.loaded, |ui| {
                    for action in actions::ALL {
                        if !action.title().to_ascii_lowercase().contains(&query) { continue; }
                        ui.horizontal(|ui| {
                            let name = ui.label(action.title());
                            if actions::REMAPPABLE.contains(&action) {
                                let mut text = self.draft.letter(action).map(|key| key.to_string()).unwrap_or_default();
                                let value = ui.add(egui::TextEdit::singleline(&mut text)
                                    .id(egui::Id::new(("shortcut-letter", action as usize)))
                                    .char_limit(1).hint_text("None").desired_width(65.)).labelled_by(name.id);
                                let clear = ui.add_enabled(self.draft.letter(action).is_some(), egui::Button::new("Clear"));
                                ui.ctx().accesskit_node_builder(clear.id, |node| node.set_label(format!("Clear {} shortcut", action.title())));
                                if value.changed() || clear.clicked() {
                                    let key = if clear.clicked() { value.request_focus(); None }
                                        else { text.chars().next().map(|letter| letter.to_ascii_uppercase()) };
                                    self.message = match self.draft.set(action, key) {
                                        Ok(()) => "Changes are not saved yet".into(), Err(error) => error,
                                    };
                                    ui.ctx().request_repaint();
                                }
                            } else { ui.label(self.active.shortcut(action)); }
                        });
                    }
                    for (title, instructions) in [
                        ("Open folder", "Use Open folder to scan supported files directly."),
                        ("Export picture", "Study → Export picture saves the completed image without app overlays."),
                        ("Export illustration", "Study → Export illustration reviews selected completed outlines and applied masks."),
                        ("Export image range / MPR grid", "Study offers up to 16 source images or all three completed thin MPR planes."),
                        ("Labels / connected regions", "Labels… offers source seed coordinates, local proposals, explicit Apply/Discard and saved masks."),
                        ("Measurements", "Measure selects a tool; Measurements… offers typed points, vertex edits and undo/redo."),
                        ("Agent access", "Optional session sharing requires a separate recipient/scope preview and your explicit Allow."),
                        ("Offline controls guide", "Help contains six manually advanced steps and a technical glossary."),
                    ] {
                        if title.to_ascii_lowercase().contains(&query) {
                            ui.label(egui::RichText::new(title).strong()); ui.label(instructions);
                        }
                    }
                    ui.separator();
                    ui.label("Fixed controls: Cmd/Ctrl+O opens files; Tab / Shift+Tab moves focus; Enter or Space activates a button. Escape closes a menu or cancels a canvas draft. Numeric fields use arrow keys while editing.");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Restore default shortcuts").clicked() {
                            self.draft = Bindings::default();
                            self.message = if self.draft == self.active { "Default shortcuts are already active" }
                                else { "Defaults selected; apply to save" }.into();
                            ui.ctx().request_repaint();
                        }
                        if ui.add_enabled(self.draft != self.active, egui::Button::new("Apply shortcuts")).clicked()
                            && let Some(worker) = &self.worker {
                            self.pending = Some(worker.submit(Request::Save(self.draft)));
                            self.message = "Saving shortcuts…".into();
                            focus_close = true;
                        }
                    });
                });
                let status = ui.push_id("shortcut-status", |ui| ui.label(&self.message)).inner;
                ui.ctx().accesskit_node_builder(status.id, |node| node.set_live(egui::accesskit::Live::Polite));
                ui.weak("Only these bindings are saved. No images, paths, study history, labels or view settings are included.");
                let close_button = ui.push_id("close-shortcuts", |ui| ui.button("Close shortcuts")).inner;
                if focus_close { close_button.request_focus(); }
                if close_button.clicked() { close = true; }
                ui.add_space(12.);
                crate::dialogs::reveal_focus(ui);
            });
        self.open = open && !close;
        if was_open && !self.open {
            self.draft = self.active;
            if let Some(id) = self.return_focus.take() {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests;
