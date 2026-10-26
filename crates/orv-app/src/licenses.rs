// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline notices from the installed package, read only when requested.
use eframe::egui::{self, RichText};
use std::{io::Read, path::Path, sync::mpsc};

const GPL: &str = include_str!("../../../LICENSE");
const SPACE: &str = include_str!("../../../assets/fonts/SpaceGrotesk-OFL.txt");
const PLEX: &str = include_str!("../../../assets/fonts/IBMPlexMono-OFL.txt");
const SEPARATOR: &str = "========================================================================";
const MAX_NOTICE_BYTES: u64 = 8 * 1024 * 1024;

struct Document {
    title: String,
    lines: Vec<String>,
}

impl Document {
    fn new(title: &str, text: &str) -> Self {
        Self {
            title: title.to_owned(),
            lines: text.lines().map(str::to_owned).collect(),
        }
    }
}

#[derive(Default)]
pub struct Licenses {
    pub open: bool,
    pending: Option<mpsc::Receiver<Result<Vec<Document>, String>>>,
    documents: Vec<Document>,
    selected: usize,
    error: Option<String>,
}

impl Licenses {
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if !self.open {
            return false;
        }
        if self.documents.is_empty() {
            self.documents = vec![
                Document::new("OpenRadView · GPL-3.0-or-later", GPL),
                Document::new("Space Grotesk · SIL Open Font License", SPACE),
                Document::new("IBM Plex Mono · SIL Open Font License", PLEX),
            ];
            let (sender, receiver) = mpsc::sync_channel(1);
            let repaint = ctx.clone();
            match std::thread::Builder::new()
                .name("orv-licenses".into())
                .spawn(move || {
                    let _ = sender.send(load_notices());
                    repaint.request_repaint();
                }) {
                Ok(_) => self.pending = Some(receiver),
                Err(_) => self.error = Some("The license reader could not start.".into()),
            }
        }
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(documents) => self.documents.extend(documents),
                        Err(message) => self.error = Some(message),
                    }
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some(
                        "The license reader stopped. Close and reopen the app to try again.".into(),
                    );
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut back = false;
        crate::dialogs::window("Third-party licenses", ctx)
            .open(&mut self.open)
            .default_width(700.)
            .default_height(510.)
            .vscroll(false)
            .show(ctx, |ui| {
                ui.label("Licenses and credits for the software, fonts and public images included with OpenRadView.");
                crate::toolbar::row(ui, |ui| {
                    back = ui.button("Back to Help").clicked();
                    egui::ComboBox::from_id_salt("license-document")
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .selected_text(&self.documents[self.selected].title)
                        .width(340.)
                        .show_ui(ui, |ui| {
                            for (index, document) in self.documents.iter().enumerate() {
                                if ui.selectable_value(&mut self.selected, index, &document.title).clicked() {
                                    ui.close();
                                }
                            }
                        });
                    if ui.button("Copy text").clicked() {
                        ctx.copy_text(self.documents[self.selected].lines.join("\n"));
                    }
                });
                if self.pending.is_some() {
                    ui.label("Loading bundled notices…");
                }
                if let Some(error) = &self.error {
                    ui.label(error);
                }
                ui.separator();
                let document = &self.documents[self.selected];
                let height = ui.text_style_height(&egui::TextStyle::Monospace);
                // Lay out only visible lines, rather than the entire notice inventory.
                egui::ScrollArea::both()
                    .id_salt(("license-text", self.selected))
                    .auto_shrink([false, false])
                    .max_height(ui.available_height().max(160.))
                    .show_rows(ui, height, document.lines.len(), |ui, rows| {
                        for row in rows {
                            ui.add(egui::Label::new(RichText::new(&document.lines[row]).monospace())
                                .selectable(true).extend());
                        }
                    });
            });
        if back {
            self.open = false;
        }
        back
    }
}

fn load_notices() -> Result<Vec<Document>, String> {
    let executable =
        std::env::current_exe().map_err(|_| "The application location could not be read.")?;
    let path = notice_path(&executable).ok_or("The application location could not be read.")?;
    read_notices(&path).map_err(|_| "The bundled third-party notices could not be read. Reinstall OpenRadView to restore them.".into())
}

fn notice_path(executable: &Path) -> Option<std::path::PathBuf> {
    let directory = executable.parent()?;
    #[cfg(target_os = "macos")]
    return Some(
        directory
            .parent()?
            .join("Resources/THIRD_PARTY_NOTICES.txt"),
    );
    #[cfg(not(target_os = "macos"))]
    Some(directory.join("THIRD-PARTY-NOTICES.txt"))
}

fn read_notices(path: &Path) -> std::io::Result<Vec<Document>> {
    let file = std::fs::File::open(path)?;
    let mut text = String::new();
    file.take(MAX_NOTICE_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_NOTICE_BYTES || text.trim().is_empty() {
        return Err(std::io::Error::other("Invalid notice size"));
    }
    let lines: Vec<_> = text.lines().collect();
    let mut documents = Vec::new();
    let mut start = 0;
    for index in 0..lines.len().saturating_sub(2) {
        let title = lines[index + 1];
        // Upstream license bodies can contain their own rows of equals signs.
        let boundary = lines[index] == SEPARATOR
            && ((title.contains('@') && lines[index + 2].starts_with("License: "))
                || title == "Bundled public image datasets (separate from the software license)");
        if boundary {
            if start < index {
                documents.push(Document::new(lines[start], &lines[start..index].join("\n")));
            }
            start = index + 1;
        }
    }
    documents.push(Document::new(lines[start], &lines[start..].join("\n")));
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bundled_sections_and_rejects_truncated_or_missing_notices() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notices.txt");
        assert!(read_notices(&path).is_err());
        std::fs::write(
            &path,
            format!("Overview\n{SEPARATOR}\nexample@1.2.3\nLicense: MIT\nOriginal copyright\n{SEPARATOR}========\n{SEPARATOR}\nOther copyright\n"),
        )
        .unwrap();
        let documents = read_notices(&path).unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[1].title, "example@1.2.3");
        assert!(documents[1].lines.iter().any(|line| line == SEPARATOR));
        assert!(
            documents[1]
                .lines
                .iter()
                .any(|line| line == "Original copyright")
        );
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_notices(&path).is_err());
        std::fs::write(&path, vec![b'a'; MAX_NOTICE_BYTES as usize + 1]).unwrap();
        assert!(read_notices(&path).is_err());
    }
}
