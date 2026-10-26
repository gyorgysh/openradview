// SPDX-License-Identifier: GPL-3.0-or-later
//! Metadata-only series catalogue and virtualized rows. No pixel decoding.
mod search;
use eframe::egui;
use orv_core::{load::LoadError, study::Study};
use std::sync::Arc;

struct Entry {
    text: String,
    searchable: String,
}
#[derive(Default)]
pub struct Catalog {
    entries: Arc<[Entry]>,
}
impl Catalog {
    /// Build on the import worker. Limit text separately from decoded images.
    pub fn build(study: &Study, cancelled: impl Fn() -> bool) -> Result<Self, LoadError> {
        if study.series.len() > 200_000 {
            return Err(LoadError::Limit);
        }
        let mut bytes = 0usize;
        let mut entries = Vec::with_capacity(study.series.len());
        for (index, series) in study.series.iter().enumerate() {
            if index % 256 == 0 && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let count = series.frame_count();
            let title = series.label();
            let text = format!(
                "{}\n{} {}",
                title,
                count,
                if count == 1 { "image" } else { "images" }
            );
            let searchable = title.to_lowercase();
            bytes = bytes
                .checked_add(text.len() + searchable.len())
                .filter(|n| *n <= 32 * 1024 * 1024)
                .ok_or(LoadError::Limit)?;
            entries.push(Entry { text, searchable });
        }
        Ok(Self {
            entries: entries.into(),
        })
    }
}
#[derive(Default)]
pub struct SeriesBrowser {
    catalog: Catalog,
    query: String,
    search: Option<search::Search>,
    rows: Option<Vec<usize>>,
    generation: u64,
    pending: bool,
}
impl SeriesBrowser {
    pub fn new(catalog: Catalog, ctx: egui::Context) -> Self {
        let search = if catalog.entries.len() > 1 {
            search::Search::start(catalog.entries.clone(), ctx)
        } else {
            None
        };
        Self {
            catalog,
            search,
            ..Default::default()
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, selected: usize) -> Option<usize> {
        if let Some(search) = &self.search
            && let Some((generation, rows)) = search.take()
            && generation == self.generation
        {
            self.rows = Some(rows);
            self.pending = false;
        }
        if self.catalog.entries.len() > 1 {
            ui.horizontal(|ui| {
                let changed = ui
                    .add_enabled(
                        self.search.is_some(),
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Filter series")
                            .char_limit(128)
                            .desired_width((ui.available_width() - 30.).max(30.)),
                    )
                    .changed();
                let clear = ui
                    .small_button("×")
                    .on_hover_text("Clear series filter")
                    .clicked();
                if clear {
                    self.query.clear();
                }
                if (changed || clear)
                    && let Some(search) = &self.search
                {
                    self.generation = search.request(self.query.clone());
                    self.pending = !self.query.trim().is_empty();
                    if !self.pending {
                        self.rows = None;
                    }
                }
            });
            let count = self
                .rows
                .as_ref()
                .map_or(self.catalog.entries.len(), Vec::len);
            // Fixed status row: filtering does not move the series list.
            ui.add(
                egui::Label::new(
                    egui::RichText::new(if self.pending {
                        "Filtering…".into()
                    } else {
                        format!("{count} / {} series", self.catalog.entries.len())
                    })
                    .small(),
                )
                .truncate(),
            );
        }
        let count = self
            .rows
            .as_ref()
            .map_or(self.catalog.entries.len(), Vec::len);
        let row_height = ui.text_style_height(&egui::TextStyle::Button)
            + ui.text_style_height(&egui::TextStyle::Small)
            + 14.;
        let mut choice = None;
        egui::ScrollArea::vertical()
            .id_salt("filtered-series")
            .max_height((ui.available_height() - 126.).max(row_height))
            .show_rows(ui, row_height, count, |ui, range| {
                for row in range {
                    let index = self.rows.as_ref().map_or(row, |rows| rows[row]);
                    if let Some(entry) = self.catalog.entries.get(index) {
                        let response = ui
                            .push_id(index, |ui| {
                                series_row(ui, &entry.text, index == selected, row_height)
                            })
                            .inner;
                        if response.on_hover_text(&entry.text).clicked() {
                            choice = Some(index);
                        }
                    }
                }
            });
        if count == 0 {
            ui.weak("No matching series. Clear the filter to see all series.");
        }
        choice
    }
}

fn series_row(ui: &mut egui::Ui, text: &str, selected: bool, height: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            text,
        )
    });
    let visuals = ui.style().interact_selectable(&response, selected);
    let painter = ui.painter_at(rect);
    if selected || response.hovered() || response.has_focus() {
        painter.rect(
            rect,
            3.,
            visuals.bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    let (title, count) = text.split_once('\n').unwrap_or((text, ""));
    let mut top = rect.top() + 5.;
    for (text, style) in [
        (title, egui::TextStyle::Button),
        (count, egui::TextStyle::Small),
    ] {
        let font = style.resolve(ui.style());
        let mut job = egui::text::LayoutJob::simple(
            text.to_owned(),
            font,
            visuals.fg_stroke.color,
            (rect.width() - 16.).max(1.),
        );
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let galley = painter.layout_job(job);
        let height = galley.size().y;
        painter.galley(
            egui::pos2(rect.left() + 8., top),
            galley,
            visuals.fg_stroke.color,
        );
        top += height + 2.;
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogue_filter_keeps_original_indices_without_materializing_pixels() {
        let mut study = orv_core::study::demo_study();
        study.series[0].modality = "CT";
        let mut second = study.series[0].clone();
        second.modality = "MR";
        second.number = 20;
        study.series.push(second);
        let mut third = study.series[0].clone();
        third.modality = "IMAGE";
        third.number = 30;
        study.series.push(third);
        let catalog = Catalog::build(&study, || false).unwrap();
        assert_eq!(
            search::matches(&catalog.entries, "mr 20", || false),
            Some(vec![1])
        );
        assert_eq!(
            search::matches(&catalog.entries, "ct", || false),
            Some(vec![0])
        );
        assert_eq!(
            search::matches(&catalog.entries, "image", || false),
            Some(vec![2])
        );
        assert_eq!(
            search::matches(&catalog.entries, "unknown", || false),
            Some(vec![])
        );
        assert!(search::matches(&catalog.entries, "ct", || true).is_none());
        assert!(matches!(
            Catalog::build(&study, || true),
            Err(LoadError::Cancelled)
        ));
    }
    #[test]
    fn a_hundred_thousand_series_only_build_visible_rows() {
        let entries = (0..100_000)
            .map(|n| Entry {
                text: format!("CT · Series {n}\n1 image"),
                searchable: format!("ct {n}"),
            })
            .collect::<Vec<_>>()
            .into();
        let mut browser = SeriesBrowser {
            catalog: Catalog { entries },
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(240., 500.),
                )),
                ..Default::default()
            },
            |ui| {
                browser.ui(ui, 0);
            },
        );
        assert!(
            output.shapes.len() < 1000,
            "offscreen rows created paint shapes"
        );
        output.textures_delta.clear();
    }
}
