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
const SEED_SLOTS: usize = MAX_SEEDS * 2;
const MAX_CELLS: usize = 1024 * 1024;

struct Stroke {
    start: usize,
    last: Option<[usize; 2]>,
    slice: usize,
    class: SeedClass,
    previous_typed: [usize; 2],
    scope: StrokeScope,
}

pub(super) struct StrokeScope {
    map: Arc<Labels>,
    label: u8,
    view: orv_core::ViewState,
    rect: egui::Rect,
}
impl StrokeScope {
    pub fn new(map: &Arc<Labels>, label: u8, view: &orv_core::ViewState, rect: egui::Rect) -> Self {
        Self {
            map: Arc::clone(map),
            label,
            view: view.clone(),
            rect,
        }
    }
    fn matches(
        &self,
        map: &Arc<Labels>,
        label: u8,
        view: &orv_core::ViewState,
        rect: egui::Rect,
    ) -> bool {
        Arc::ptr_eq(&self.map, map)
            && self.label == label
            && self.rect == rect
            && self.view.plane == view.plane
            && self.view.slice == view.slice
            && self.view.zoom == view.zoom
            && self.view.pan == view.pan
            && self.view.transform == view.transform
    }
}

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
    seed_index: Vec<u16>,
    groups: Vec<usize>,
    stroke: Option<Stroke>,
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
            seed_index: Vec::new(),
            groups: Vec::new(),
            stroke: None,
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
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.preview = None;
    }
    fn ensure_storage(&mut self) -> Result<(), &'static str> {
        if self.seed_memory.is_none() {
            let bytes = MAX_SEEDS
                * (std::mem::size_of::<SeedPoint>() + std::mem::size_of::<usize>())
                + SEED_SLOTS * std::mem::size_of::<u16>();
            let reservation = MemoryBudget::volumes()
                .reserve(bytes as u64)
                .ok_or("Not enough memory for seed points.")?;
            let mut seeds = Vec::new();
            let mut groups = Vec::new();
            let mut index = Vec::new();
            seeds
                .try_reserve_exact(MAX_SEEDS)
                .map_err(|_| "Not enough memory for seed points.")?;
            groups
                .try_reserve_exact(MAX_SEEDS)
                .map_err(|_| "Not enough memory for seed points.")?;
            index
                .try_reserve_exact(SEED_SLOTS)
                .map_err(|_| "Not enough memory for seed points.")?;
            if seeds.capacity() != MAX_SEEDS
                || groups.capacity() != MAX_SEEDS
                || index.capacity() != SEED_SLOTS
            {
                return Err("Not enough memory for seed points.");
            }
            index.resize(SEED_SLOTS, 0);
            self.seeds = seeds;
            self.groups = groups;
            self.seed_index = index;
            self.seed_memory = Some(reservation);
        }
        Ok(())
    }
    fn hash(seed: SeedPoint) -> usize {
        fn mix(mut x: u64) -> u64 {
            x ^= x >> 30;
            x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x ^= x >> 27;
            x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        }
        let class = u64::from(seed.class == SeedClass::Exclude);
        (mix(seed.position[0] as u64 ^ mix(seed.position[1] as u64) ^ class) as usize)
            & (SEED_SLOTS - 1)
    }
    fn insert(&mut self, seed: SeedPoint) -> Result<bool, &'static str> {
        let mut slot = Self::hash(seed);
        for _ in 0..SEED_SLOTS {
            let entry = self.seed_index[slot];
            if entry == 0 {
                if self.seeds.len() >= MAX_SEEDS {
                    return Err("Seed limit reached; the current stroke was discarded.");
                }
                self.seeds.push(seed);
                self.seed_index[slot] =
                    u16::try_from(self.seeds.len()).map_err(|_| "Seed limit reached.")?;
                self.version = self.version.wrapping_add(1);
                return Ok(true);
            }
            if self.seeds[usize::from(entry - 1)] == seed {
                return Ok(false);
            }
            slot = (slot + 1) & (SEED_SLOTS - 1);
        }
        Err("Seed index is full.")
    }
    fn rebuild_index(&mut self) {
        self.seed_index.fill(0);
        for (id, seed) in self.seeds.iter().copied().enumerate() {
            let mut slot = Self::hash(seed);
            while self.seed_index[slot] != 0 {
                slot = (slot + 1) & (SEED_SLOTS - 1);
            }
            self.seed_index[slot] = (id + 1) as u16;
        }
    }
    fn select_slice(&mut self, z: usize) {
        if self.slice != Some(z) {
            self.clear_seeds();
            self.slice = Some(z);
        }
    }
    fn clear_seeds(&mut self) {
        self.seeds.clear();
        self.groups.clear();
        self.seed_index.fill(0);
        self.stroke = None;
        self.error = None;
        self.slice = None;
        self.version = self.version.wrapping_add(1);
    }
    pub fn add(&mut self, point: [usize; 3], class: SeedClass) {
        if let Err(error) = self.ensure_storage() {
            self.error = Some(error);
            return;
        }
        self.error = None;
        self.select_slice(point[2]);
        let start = self.seeds.len();
        match self.insert(SeedPoint {
            position: [point[0], point[1]],
            class,
        }) {
            Ok(true) => self.groups.push(start),
            Ok(false) => (),
            Err(error) => self.error = Some(error),
        }
        self.typed = [point[0] + 1, point[1] + 1];
    }
    fn remove_last(&mut self) {
        if let Some(start) = self.groups.pop() {
            self.seeds.truncate(start);
            self.rebuild_index();
            self.error = None;
            self.version = self.version.wrapping_add(1);
        }
    }
    pub fn cancel_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take() {
            self.seeds.truncate(stroke.start);
            self.typed = stroke.previous_typed;
            self.rebuild_index();
            self.version = self.version.wrapping_add(1);
        }
    }
    pub fn begin_stroke(
        &mut self,
        point: [usize; 3],
        class: SeedClass,
        scope: StrokeScope,
    ) -> bool {
        if let Err(error) = self.ensure_storage() {
            self.error = Some(error);
            return false;
        }
        self.error = None;
        self.select_slice(point[2]);
        self.stroke = Some(Stroke {
            start: self.seeds.len(),
            last: None,
            slice: point[2],
            class,
            previous_typed: self.typed,
            scope,
        });
        self.extend_stroke(point)
    }
    pub fn extend_stroke(&mut self, point: [usize; 3]) -> bool {
        let Some(stroke) = &self.stroke else {
            return false;
        };
        if stroke.slice != point[2] {
            self.cancel_stroke();
            return false;
        }
        let target = [point[0], point[1]];
        let from = stroke.last.unwrap_or(target);
        let class = stroke.class;
        let dx = from[0].abs_diff(target[0]);
        let dy = from[1].abs_diff(target[1]);
        if dx.max(dy) >= MAX_SEEDS {
            self.cancel_stroke();
            self.error = Some("Stroke is too long; draw shorter hints.");
            return false;
        }
        let (mut x, mut y) = (from[0] as i64, from[1] as i64);
        let (tx, ty) = (target[0] as i64, target[1] as i64);
        let (dx, dy) = (dx as i64, -(dy as i64));
        let (sx, sy) = ((tx - x).signum(), (ty - y).signum());
        let mut error = dx + dy;
        loop {
            if let Err(message) = self.insert(SeedPoint {
                position: [x as usize, y as usize],
                class,
            }) {
                self.cancel_stroke();
                self.error = Some(message);
                return false;
            }
            if x == tx && y == ty {
                break;
            }
            let twice = 2 * error;
            if twice >= dy {
                error += dy;
                x += sx;
            }
            if twice <= dx {
                error += dx;
                y += sy;
            }
        }
        if let Some(stroke) = &mut self.stroke {
            stroke.last = Some(target);
        }
        self.typed = [point[0] + 1, point[1] + 1];
        true
    }
    pub fn break_stroke(&mut self) {
        if let Some(stroke) = &mut self.stroke {
            stroke.last = None;
        }
    }
    pub fn drawing(&self) -> bool {
        self.stroke.is_some()
    }
    pub fn drawing_on(
        &self,
        map: &Arc<Labels>,
        label: u8,
        slice: usize,
        view: &orv_core::ViewState,
        rect: egui::Rect,
    ) -> bool {
        self.stroke.as_ref().is_some_and(|stroke| {
            stroke.slice == slice && stroke.scope.matches(map, label, view, rect)
        })
    }
    pub fn markers(&self, slice: usize) -> &[SeedPoint] {
        if self.slice == Some(slice) {
            &self.seeds
        } else {
            &[]
        }
    }
    pub fn finish_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take()
            && self.seeds.len() > stroke.start
        {
            self.groups.push(stroke.start);
        }
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
    pub fn sync(&mut self, map: &Arc<Labels>, label: u8, slice: Option<usize>) -> bool {
        let cancelled = self.stroke.as_ref().is_some_and(|stroke| {
            !Arc::ptr_eq(&stroke.scope.map, map)
                || stroke.scope.label != label
                || Some(stroke.slice) != slice
        });
        if cancelled {
            self.cancel_stroke();
        }
        let source = map.source_arc();
        let source_key = Arc::downgrade(&source);
        if self
            .source
            .as_ref()
            .is_none_or(|old| !old.ptr_eq(&source_key))
        {
            self.source = Some(source_key);
            self.clear_seeds();
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
        cancelled
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
            ui.small("Drag or click to add a hint. Squares include; crosses exclude. Remove last hint removes one stroke.");
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
                for (class, text) in [(SeedClass::Include,"Draw include hint"),(SeedClass::Exclude,"Draw exclude hint")] {
                    if ui.add_enabled(slice.is_some() && self.regions.seeded.seeds.len() < MAX_SEEDS,egui::Button::new(text)).clicked() {
                        self.paint=false; self.clear_brush(); self.regions.invalidate();
                        self.regions.seed_picking=Some(class); self.regions.picking=true; self.open=false;
                    }
                }
            });
            let include=self.regions.seeded.seeds.iter().filter(|s|s.class==SeedClass::Include).count();
            let exclude=self.regions.seeded.seeds.iter().filter(|s|s.class==SeedClass::Exclude).count();
            ui.small(format!("{include} include and {exclude} exclude seeds · image {}",self.regions.seeded.slice.map_or_else(||"not selected".to_owned(),|z|(z+1).to_string())));
            if let Some(error)=self.regions.seeded.error {ui.label(error);}
            for (index, &start) in self.regions.seeded.groups.iter().take(6).enumerate() {
                let seed=self.regions.seeded.seeds[start];
                let end=self.regions.seeded.groups.get(index+1).copied().unwrap_or(self.regions.seeded.seeds.len());
                let class=if seed.class==SeedClass::Include {"Include"} else {"Exclude"};
                let count=end-start;
                let unit=if count==1 {"point"} else {"points"};
                ui.small(format!("{class}: {count} {unit}, starting at column {}, row {}",seed.position[0]+1,seed.position[1]+1));
            }
            if self.regions.seeded.groups.len()>6 {ui.small(format!("and {} more hints",self.regions.seeded.groups.len()-6));}
            if ui.add_enabled(!self.regions.seeded.seeds.is_empty(),egui::Button::new("Remove last hint")).clicked() {
                self.regions.seeded.remove_last();
                self.regions.invalidate();
            }
            if ui.add_enabled(!self.regions.seeded.seeds.is_empty(),egui::Button::new("Clear seeds")).clicked() {
                self.regions.seeded.clear_seeds();
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
