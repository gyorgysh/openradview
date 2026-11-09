// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable actions, shared labels and strict, unmodified local tool bindings.
use crate::viewport::Tool;
use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Action {
    Pan,
    Zoom,
    Browse,
    Contrast,
    Crosshair,
    Distance,
    Angle,
    Rectangle,
    Ellipse,
    Polygon,
    Edit,
    Fit,
    Invert,
    Next,
    Previous,
    OpenFiles,
}
pub(crate) const REMAPPABLE: [Action; 13] = [
    Action::Pan,
    Action::Zoom,
    Action::Browse,
    Action::Contrast,
    Action::Crosshair,
    Action::Distance,
    Action::Angle,
    Action::Rectangle,
    Action::Ellipse,
    Action::Polygon,
    Action::Edit,
    Action::Fit,
    Action::Invert,
];
pub(crate) const ALL: [Action; 16] = [
    Action::Pan,
    Action::Zoom,
    Action::Browse,
    Action::Contrast,
    Action::Crosshair,
    Action::Distance,
    Action::Angle,
    Action::Rectangle,
    Action::Ellipse,
    Action::Polygon,
    Action::Edit,
    Action::Fit,
    Action::Invert,
    Action::Next,
    Action::Previous,
    Action::OpenFiles,
];
impl Action {
    pub fn title(self) -> &'static str {
        match self {
            Self::Pan => "Pan",
            Self::Zoom => "Zoom",
            Self::Browse => "Images",
            Self::Contrast => "Contrast",
            Self::Crosshair => "Crosshair",
            Self::Distance => "Distance",
            Self::Angle => "Angle",
            Self::Rectangle => "Rectangle ROI",
            Self::Ellipse => "Ellipse ROI",
            Self::Polygon => "Polygon ROI",
            Self::Edit => "Edit measurements",
            Self::Fit => "Fit",
            Self::Invert => "Invert",
            Self::Next => "Next image / plane",
            Self::Previous => "Previous image / plane",
            Self::OpenFiles => "Open files",
        }
    }
    pub fn tool(self) -> Option<Tool> {
        Some(match self {
            Self::Pan => Tool::Pan,
            Self::Zoom => Tool::Zoom,
            Self::Browse => Tool::Browse,
            Self::Contrast => Tool::Window,
            Self::Crosshair => Tool::Crosshair,
            Self::Distance => Tool::Distance,
            Self::Angle => Tool::Angle,
            Self::Rectangle => Tool::Roi,
            Self::Ellipse => Tool::EllipseRoi,
            Self::Polygon => Tool::PolygonRoi,
            Self::Edit => Tool::EditMarks,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bindings {
    keys: [Option<char>; 13],
}
impl Default for Bindings {
    fn default() -> Self {
        Self {
            keys: [
                Some('P'),
                Some('Z'),
                Some('S'),
                Some('W'),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some('R'),
                Some('I'),
            ],
        }
    }
}
impl Bindings {
    pub fn letter(self, action: Action) -> Option<char> {
        self.keys.get(action as usize).copied().flatten()
    }
    pub fn set(&mut self, action: Action, key: Option<char>) -> Result<(), String> {
        if let Some(key) = key
            && let Some(other) = self
                .keys
                .iter()
                .enumerate()
                .find(|(index, current)| *index != action as usize && **current == Some(key))
        {
            return Err(format!(
                "{key} is already assigned to {}",
                REMAPPABLE[other.0].title()
            ));
        }
        let mut candidate = *self;
        *candidate
            .keys
            .get_mut(action as usize)
            .ok_or("This binding is fixed")? = key;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    pub fn validate(self) -> Result<(), String> {
        for (i, key) in self.keys.iter().enumerate() {
            if let Some(key) = key {
                if !key.is_ascii_uppercase() {
                    return Err("Choose one letter from A to Z, or None".into());
                }
                if let Some(other) = self.keys[..i].iter().position(|k| k == &Some(*key)) {
                    return Err(format!(
                        "{key} is already assigned to {}",
                        REMAPPABLE[other].title()
                    ));
                }
            }
        }
        Ok(())
    }
    fn key(self, action: Action) -> Option<egui::Key> {
        match action {
            Action::Next => Some(egui::Key::ArrowDown),
            Action::Previous => Some(egui::Key::ArrowUp),
            Action::OpenFiles => Some(egui::Key::O),
            _ => self
                .letter(action)
                .and_then(|key| egui::Key::from_name(&key.to_string())),
        }
    }
    pub fn shortcut(self, action: Action) -> String {
        match action {
            Action::Next => "↓".into(),
            Action::Previous => "↑".into(),
            Action::OpenFiles => "Cmd/Ctrl+O".into(),
            _ => self
                .letter(action)
                .map(|k| k.to_string())
                .unwrap_or_else(|| "None".into()),
        }
    }
}
fn id() -> egui::Id {
    egui::Id::new("local-action-bindings")
}
pub(crate) fn install(ctx: &egui::Context, bindings: Bindings) {
    ctx.data_mut(|data| data.insert_temp(id(), bindings));
}
pub(crate) fn bindings(ctx: &egui::Context) -> Bindings {
    ctx.data(|data| data.get_temp::<Bindings>(id()))
        .unwrap_or_default()
}
pub(crate) fn label(ctx: &egui::Context, action: Action) -> String {
    let bindings = bindings(ctx);
    match bindings.key(action) {
        Some(_) => format!("{} [{}]", action.title(), bindings.shortcut(action)),
        None => action.title().into(),
    }
}
pub(crate) fn view_input_enabled(ctx: &egui::Context, enabled: bool) {
    ctx.data_mut(|data| data.insert_temp(id().with("view-input"), enabled));
}
fn input_enabled(ui: &egui::Ui, action: Action) -> bool {
    ui.ctx()
        .data(|data| data.get_temp::<bool>(id().with("view-input")))
        .unwrap_or(true)
        && ui.is_enabled()
        && (action == Action::OpenFiles || !ui.ctx().egui_wants_keyboard_input())
        && !egui::Popup::is_any_open(ui.ctx())
}
pub(crate) fn pressed(ui: &egui::Ui, action: Action) -> bool {
    if !input_enabled(ui, action) {
        return false;
    }
    let Some(key) = bindings(ui.ctx()).key(action) else {
        return false;
    };
    // egui's logical modifier matcher intentionally ignores extra Shift/Alt.
    // Tool letters must never also fire for an OS or text-editing shortcut.
    ui.input_mut(|input| {
        let mut found = false;
        input.events.retain(|event| {
            let matches = matches!(event, egui::Event::Key {
                key: event_key, pressed: true, modifiers, ..
            } if *event_key == key && match action {
                Action::OpenFiles => modifiers.command && !modifiers.alt && !modifiers.shift
                    && !(modifiers.mac_cmd && modifiers.ctrl),
                _ => *modifiers == egui::Modifiers::NONE,
            });
            found |= matches;
            !matches
        });
        found
    })
}
pub(crate) fn navigate(ui: &egui::Ui, initial: u32, max: u32) -> u32 {
    if !input_enabled(ui, Action::Next) {
        return initial;
    }
    let bindings = bindings(ui.ctx());
    let (Some(next), Some(previous)) = (bindings.key(Action::Next), bindings.key(Action::Previous))
    else {
        return initial;
    };
    let mut slice = initial;
    ui.input_mut(|input| {
        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
                ..
            } = event
            {
                if *key == next {
                    slice = slice.saturating_add(1).min(max);
                    return false;
                }
                if *key == previous {
                    slice = slice.saturating_sub(1);
                    return false;
                }
            }
            true
        });
    });
    slice
}
pub(crate) fn tool_input(
    ui: &egui::Ui,
    tool: &mut Tool,
    scalar: bool,
    crosshair: bool,
    measurements: bool,
) {
    for action in REMAPPABLE {
        let Some(next) = action.tool() else { continue };
        if (scalar || next != Tool::Window)
            && (crosshair || next != Tool::Crosshair)
            && (measurements || !next.measures())
            && pressed(ui, action)
        {
            *tool = next;
        }
    }
}

#[cfg(test)]
mod tests;
