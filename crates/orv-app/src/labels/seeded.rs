// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-slice seed entry and private, revision-bound region previews.
use super::{LabelUi, Operation};
use eframe::egui;
use orv_core::{
    labels::{Labels, SeedClass, SeedPoint, SeededRequest, SeededSummary},
    resources::{MemoryBudget, Reservation},
    volume::Volume,
};
use std::sync::{Arc, Weak};

const MAX_SEEDS: usize = 8192;
const MAX_CELLS: usize = 1024 * 1024;

pub(super) struct Preview {
    pub base: Arc<Labels>,
    pub labels: Arc<Labels>,
    pub request: SeededRequest,
    pub summary: SeededSummary,
    pub version: u64,
    pub epoch: u64,
}
struct Inputs {
    map: Weak<Labels>,
    request: Option<SeededRequest>,
    version: u64,
}
pub(super) struct Seeded {
    source: Option<Weak<Volume>>,
    slice: Option<usize>,
    seeds: Vec<SeedPoint>,
    seed_memory: Option<Reservation>,
    error: Option<&'static str>,
    origin: [usize; 2],
    size: [usize; 2],
    typed: [usize; 2],
    iterations: u32,
    version: u64,
    epoch: u64,
    inputs: Option<Inputs>,
    pub preview: Option<Preview>,
}
impl Default for Seeded {
    fn default() -> Self {
        Self {
            source: None,
            slice: None,
            seeds: Vec::new(),
            seed_memory: None,
            error: None,
            origin: [1, 1],
            size: [0, 0],
            typed: [1, 1],
            iterations: 512,
            version: 0,
            epoch: 0,
            inputs: None,
            preview: None,
        }
    }
}
impl Seeded {
    pub fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.preview = None;
    }
    pub fn add(&mut self, point: [usize; 3], class: SeedClass) {
        if self.seed_memory.is_none() {
            let bytes = MAX_SEEDS * std::mem::size_of::<SeedPoint>();
            let Some(reservation) = MemoryBudget::volumes().reserve(bytes as u64) else {
                self.error = Some("Not enough memory for seed points.");
                return;
            };
            let mut seeds = Vec::new();
            if seeds.try_reserve_exact(MAX_SEEDS).is_err() || seeds.capacity() != MAX_SEEDS {
                self.error = Some("Not enough memory for seed points.");
                return;
            }
            self.seeds = seeds;
            self.seed_memory = Some(reservation);
        }
        self.error = None;
        if self.slice != Some(point[2]) {
            self.seeds.clear();
            self.slice = Some(point[2]);
        }
        let seed = SeedPoint {
            position: [point[0], point[1]],
            class,
        };
        if self.seeds.len() < MAX_SEEDS && !self.seeds.contains(&seed) {
            self.seeds.push(seed);
            self.version = self.version.wrapping_add(1);
        }
        self.typed = [point[0] + 1, point[1] + 1];
    }
    fn copy_for_worker(&self) -> Result<(Vec<SeedPoint>, Reservation), &'static str> {
        let bytes = self
            .seeds
            .len()
            .checked_mul(std::mem::size_of::<SeedPoint>())
            .ok_or("Seed set is too large.")?;
        let reservation = MemoryBudget::volumes()
            .reserve(bytes as u64)
            .ok_or("Not enough memory for seed points.")?;
        let mut seeds = Vec::new();
        seeds
            .try_reserve_exact(self.seeds.len())
            .map_err(|_| "Not enough memory for seed points.")?;
        if seeds.capacity() != self.seeds.len() {
            return Err("Not enough memory for seed points.");
        }
        seeds.extend_from_slice(&self.seeds);
        Ok((seeds, reservation))
    }
    fn request(&self, map: &Labels, label: u8, slice: Option<usize>) -> Option<SeededRequest> {
        let z = slice?;
        if map.source().is_color()
            || self.slice != Some(z)
            || label == 0
            || !self.seeds.iter().any(|s| s.class == SeedClass::Include)
            || !self.seeds.iter().any(|s| s.class == SeedClass::Exclude)
        {
            return None;
        }
        let [width, height, depth] = map.source().dimensions();
        let origin = [
            self.origin[0].checked_sub(1)?,
            self.origin[1].checked_sub(1)?,
        ];
        let [w, h] = self.size;
        if z >= depth
            || w == 0
            || h == 0
            || w.checked_mul(h)? > MAX_CELLS
            || origin[0].checked_add(w)? > width
            || origin[1].checked_add(h)? > height
            || !(1..=4096).contains(&self.iterations)
        {
            return None;
        }
        if self.seeds.iter().any(|seed| {
            seed.position[0] < origin[0]
                || seed.position[1] < origin[1]
                || seed.position[0] >= origin[0] + w
                || seed.position[1] >= origin[1] + h
        }) {
            return None;
        }
        Some(SeededRequest {
            slice: z,
            origin,
            size: self.size,
            label,
            max_iterations: self.iterations,
        })
    }
    pub fn sync(&mut self, map: &Arc<Labels>, label: u8, slice: Option<usize>) {
        let source = map.source_arc();
        let source_key = Arc::downgrade(&source);
        if self
            .source
            .as_ref()
            .is_none_or(|old| !old.ptr_eq(&source_key))
        {
            self.source = Some(source_key);
            self.seeds.clear();
            self.slice = None;
            self.origin = [1, 1];
            let [w, h, _] = source.dimensions();
            self.size = [w, h];
            self.typed = [1, 1];
            self.version = self.version.wrapping_add(1);
            self.invalidate();
        }
        let map_key = Arc::downgrade(map);
        let request = self.request(map, label, slice);
        if self.inputs.as_ref().is_none_or(|old| {
            !old.map.ptr_eq(&map_key) || old.request != request || old.version != self.version
        }) {
            self.invalidate();
            self.inputs = Some(Inputs {
                map: map_key,
                request,
                version: self.version,
            });
        }
    }
    pub fn accept(&self, preview: &Preview, map: &Arc<Labels>) -> bool {
        preview.epoch == self.epoch
            && Arc::ptr_eq(&preview.base, map)
            && self.inputs.as_ref().is_some_and(|input| {
                input.request == Some(preview.request)
                    && input.version == preview.version
                    && input.map.ptr_eq(&Arc::downgrade(map))
            })
    }
}
impl LabelUi {
    pub(super) fn seeded_controls(
        &mut self,
        ui: &mut egui::Ui,
        slice: Option<usize>,
        ctx: &egui::Context,
    ) {
        let Some(map) = self.current.clone() else {
            return;
        };
        self.regions.seeded.sync(&map, self.label, slice);
        ui.collapsing("Include / exclude seeds", |ui| {
            ui.label("Suggest a region from include and exclude hints on one source image. Check the preview before applying; this does not recognize anatomy or disease.");
            ui.small("Other label IDs and missing samples block propagation. Seeds on a different source image replace the previous seed set.");
            let [width, height, _] = map.source().dimensions();
            let seeded = &mut self.regions.seeded;
            ui.label("Rectangle on this source image (1-based origin and size):");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut seeded.origin[0]).range(1..=width).prefix("Column "));
                ui.add(egui::DragValue::new(&mut seeded.origin[1]).range(1..=height).prefix("Row "));
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut seeded.size[0]).range(1..=width).prefix("Width "));
                ui.add(egui::DragValue::new(&mut seeded.size[1]).range(1..=height).prefix("Height "));
            });
            ui.small("The rectangle can contain at most 1,048,576 cells; complex regions may exceed the work limit.");
            ui.add(egui::DragValue::new(&mut seeded.iterations).range(1..=4096).prefix("Iteration limit "));
            if map.source().is_color() {ui.small("Seed propagation requires a scalar source image.");}
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut seeded.typed[0]).range(1..=width).prefix("Seed column "));
                ui.add(egui::DragValue::new(&mut seeded.typed[1]).range(1..=height).prefix("row "));
            });
            ui.horizontal(|ui| {
                for (class, text) in [(SeedClass::Include,"Add include seed"),(SeedClass::Exclude,"Add exclude seed")] {
                    if ui.add_enabled(slice.is_some() && self.regions.seeded.seeds.len() < MAX_SEEDS,egui::Button::new(text)).clicked() && let Some(z)=slice {
                        let [x,y]=self.regions.seeded.typed;
                        self.regions.seeded.add([x-1,y-1,z],class);
                        self.regions.invalidate();
                    }
                }
            });
            ui.horizontal(|ui| {
                for (class, text) in [(SeedClass::Include,"Pick include on image"),(SeedClass::Exclude,"Pick exclude on image")] {
                    if ui.add_enabled(slice.is_some() && self.regions.seeded.seeds.len() < MAX_SEEDS,egui::Button::new(text)).clicked() {
                        self.paint=false; self.stroke.clear(); self.regions.invalidate();
                        self.regions.seed_picking=Some(class); self.regions.picking=true; self.open=false;
                    }
                }
            });
            let include=self.regions.seeded.seeds.iter().filter(|s|s.class==SeedClass::Include).count();
            let exclude=self.regions.seeded.seeds.iter().filter(|s|s.class==SeedClass::Exclude).count();
            ui.small(format!("{include} include and {exclude} exclude seeds · image {}",self.regions.seeded.slice.map_or_else(||"not selected".to_owned(),|z|(z+1).to_string())));
            if let Some(error)=self.regions.seeded.error {ui.label(error);}
            for seed in self.regions.seeded.seeds.iter().take(12) {
                let class=if seed.class==SeedClass::Include {"Include"} else {"Exclude"};
                ui.small(format!("{class}: column {}, row {}",seed.position[0]+1,seed.position[1]+1));
            }
            if self.regions.seeded.seeds.len()>12 {ui.small(format!("and {} more seeds",self.regions.seeded.seeds.len()-12));}
            if ui.add_enabled(!self.regions.seeded.seeds.is_empty(),egui::Button::new("Remove last seed")).clicked() {
                self.regions.seeded.seeds.pop();
                self.regions.seeded.version=self.regions.seeded.version.wrapping_add(1);
                self.regions.invalidate();
            }
            if ui.add_enabled(!self.regions.seeded.seeds.is_empty(),egui::Button::new("Clear seeds")).clicked() {
                self.regions.seeded.seeds.clear(); self.regions.seeded.slice=None;
                self.regions.seeded.version=self.regions.seeded.version.wrapping_add(1);
                self.regions.invalidate();
            }
            self.regions.seeded.sync(&map,self.label,slice);
            let request=self.regions.seeded.request(&map,self.label,slice);
            if request.is_none() { ui.small("Choose both seed types inside a valid rectangle on this image."); }
            if ui.add_enabled(request.is_some(),egui::Button::new("Preview seeded region")).clicked() && let Some(request)=request {
                self.paint=false; self.regions.invalidate(); self.visible=true;
                self.display.visible_ids[self.label as usize]=true;
                let seeded=&self.regions.seeded;
                match seeded.copy_for_worker() {
                    Ok((seeds, reservation)) => self.start(Operation::SeededPreview(map.clone(),request,seeds,reservation,seeded.version,seeded.epoch),ctx),
                    Err(error) => self.message=error.into(),
                }
            }
            if let Some(preview)=&self.regions.seeded.preview {
                ui.label(format!("PREVIEW ONLY · add {} cells, remove {} cells · {} iterations",preview.summary.added,preview.summary.removed,preview.summary.iterations));
                ui.weak("Counts and exports use the applied map. Inspect the overlay before applying.");
                let changes=preview.summary.added+preview.summary.removed>0;
                ui.horizontal(|ui| {
                    if ui.add_enabled(changes,egui::Button::new("Apply seeded region")).clicked() { self.apply_seeded_preview(slice); }
                    if ui.button("Discard seeded preview").clicked() { self.regions.invalidate(); }
                });
            }
        });
        self.regions.seeded.sync(&map, self.label, slice);
    }
    pub(super) fn apply_seeded_preview(&mut self, slice: Option<usize>) {
        let Some(map) = self.current.clone() else {
            return;
        };
        self.regions.seeded.sync(&map, self.label, slice);
        if let Some(preview) = self.regions.seeded.preview.take()
            && self.regions.seeded.accept(&preview, &map)
            && preview.summary.added + preview.summary.removed > 0
        {
            self.commit_change(preview.labels);
        }
    }
}
#[cfg(test)]
mod tests;
