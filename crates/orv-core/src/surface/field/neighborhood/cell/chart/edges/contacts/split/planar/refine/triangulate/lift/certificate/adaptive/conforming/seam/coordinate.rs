// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded fixed-point coordination over explicitly supplied source cells.
use super::*;

const MAX_CELLS: usize = 256;
const MAX_FACES: usize = 3 * MAX_CELLS;
const MAX_ROUNDS: u8 = 16;
const MAX_VISITS: usize = 16_384;
const MAX_UPDATES: usize = 1024;
const MAX_DIRECTORY: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSeamLimits {
    pub max_cells: usize,
    pub max_faces: usize,
    pub max_rounds: u8,
    pub max_face_visits: usize,
    pub max_updates: usize,
    pub partition: FacePartitionLimits,
    pub apply: AppliedFaceLimits,
    pub promotion: FacePromotionLimits,
    /// Additional retained directory; cell generations and scratch are separate.
    pub max_bytes: u64,
}
impl Default for CellSeamLimits {
    fn default() -> Self {
        Self {
            max_cells: MAX_CELLS,
            max_faces: MAX_FACES,
            max_rounds: MAX_ROUNDS,
            max_face_visits: MAX_VISITS,
            max_updates: MAX_UPDATES,
            partition: FacePartitionLimits::default(),
            apply: AppliedFaceLimits::default(),
            promotion: FacePromotionLimits::default(),
            max_bytes: MAX_DIRECTORY,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellSeamStop {
    ProjectionMismatch,
    ProjectionEndFace,
    PartitionGeometry,
    PartitionLimit,
    ApplyGeometry,
    ApplyLimit,
    PromotionGeometry,
    PromotionLimit,
    UpdateLimit,
    NotRechecked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellFaceState {
    /// Candidate edges match within this supplied pair; not a full surface claim.
    CandidateAligned {
        points: u32,
        spans: u32,
    },
    /// Missing point or interval coverage remains on at least one candidate side.
    Uncovered {
        points: u32,
        spans: u32,
        missing_points: u32,
        missing_spans: u32,
    },
    NoCandidateCoverage,
    Unresolved {
        reason: CellSeamStop,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellFaceReport {
    pub cells: [u16; 2],
    pub face: FaceKey,
    pub state: CellFaceState,
    observed: [u32; 2],
}
/// An immutable local batch. Cells outside the supplied list and all noncandidate
/// surface strata remain outside this coordination and are not certified.
pub struct CellSeamBatch {
    cells: Vec<Arc<ConformingSurfaceMesh>>,
    faces: Vec<CellFaceReport>,
    rounds: u8,
    visits: usize,
    updates: usize,
    bytes: u64,
    _reservation: Reservation,
}
impl CellSeamBatch {
    pub fn cells(&self) -> &[Arc<ConformingSurfaceMesh>] {
        &self.cells
    }
    pub fn faces(&self) -> &[CellFaceReport] {
        &self.faces
    }
    pub fn rounds(&self) -> u8 {
        self.rounds
    }
    pub fn face_visits(&self) -> usize {
        self.visits
    }
    pub fn updates(&self) -> usize {
        self.updates
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
    /// Only the represented candidate edges on supplied, nonempty faces match.
    pub fn candidate_faces_aligned(&self) -> bool {
        !self.faces.is_empty()
            && self
                .faces
                .iter()
                .all(|f| matches!(f.state, CellFaceState::CandidateAligned { .. }))
    }
}
fn adjacent(a: [u32; 3], b: [u32; 3]) -> Option<(usize, bool)> {
    let mut found = None;
    for axis in 0..3 {
        if a[axis] == b[axis] {
            continue;
        }
        let upper = if a[axis].checked_add(1) == Some(b[axis]) {
            true
        } else if b[axis].checked_add(1) == Some(a[axis]) {
            false
        } else {
            return None;
        };
        if found.is_some() {
            return None;
        }
        found = Some((axis, upper));
    }
    found
}
fn face_result(
    partition: &SharedFacePartition,
    cancel: &impl Fn() -> bool,
) -> Result<CellFaceState, SurfaceError> {
    let points = u32::try_from(partition.points.len()).map_err(|_| SurfaceError::Limit)?;
    let spans = u32::try_from(partition.spans.len()).map_err(|_| SurfaceError::Limit)?;
    if points == 0 || spans == 0 {
        return Ok(CellFaceState::NoCandidateCoverage);
    }
    let mut missing_points = 0u32;
    let mut missing_spans = 0u32;
    for point in &partition.points {
        check(cancel)?;
        missing_points += u32::from(point.placements.contains(&FacePlacement::Absent));
    }
    for span in &partition.spans {
        check(cancel)?;
        missing_spans += u32::from(span.coverage.contains(&FaceSpanCoverage::Absent));
    }
    Ok(if missing_points == 0 && missing_spans == 0 {
        CellFaceState::CandidateAligned { points, spans }
    } else {
        CellFaceState::Uncovered {
            points,
            spans,
            missing_points,
            missing_spans,
        }
    })
}
fn stage_error(
    error: SurfaceError,
    geometry: CellSeamStop,
    limit: CellSeamStop,
) -> Result<CellFaceState, SurfaceError> {
    match error {
        SurfaceError::Geometry => Ok(CellFaceState::Unresolved { reason: geometry }),
        SurfaceError::Limit => Ok(CellFaceState::Unresolved { reason: limit }),
        other => Err(other),
    }
}
impl CellSeamBatch {
    /// Coordinate faces among these immutable source cells. Successful updates
    /// are held privately until the entire batch is returned; unlisted neighbors,
    /// noncandidate strata, physical error and rendering are not covered.
    pub fn coordinate(
        inputs: &[Arc<ConformingSurfaceMesh>],
        limits: CellSeamLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        Self::coordinate_with_budget(inputs, limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn coordinate_with_budget(
        inputs: &[Arc<ConformingSurfaceMesh>],
        limits: CellSeamLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        check(&cancel)?;
        if inputs.is_empty()
            || limits.max_cells > MAX_CELLS
            || inputs.len() > limits.max_cells
            || limits.max_faces > MAX_FACES
            || limits.max_rounds == 0
            || limits.max_rounds > MAX_ROUNDS
            || limits.max_face_visits > MAX_VISITS
            || limits.max_updates > MAX_UPDATES
            || limits.max_bytes > MAX_DIRECTORY
        {
            return Err(SurfaceError::Limit);
        }
        let first = chart(&inputs[0]).graph().field();
        for cell in inputs {
            check(&cancel)?;
            let field = chart(cell).graph().field();
            if field.key() != first.key()
                || field.revision() != first.revision()
                || !Arc::ptr_eq(field.source(), first.source())
            {
                return Err(SurfaceError::Geometry);
            }
        }
        let mut face_count = 0usize;
        for i in 0..inputs.len() {
            for j in i + 1..inputs.len() {
                check(&cancel)?;
                let a = chart(&inputs[i]).graph().cell().origin();
                let b = chart(&inputs[j]).graph().cell().origin();
                if a == b {
                    return Err(SurfaceError::Geometry);
                }
                if adjacent(a, b).is_some() {
                    face_count += 1;
                }
            }
        }
        if face_count > limits.max_faces {
            return Err(SurfaceError::Limit);
        }
        let bytes = size(
            &[
                (
                    inputs.len(),
                    std::mem::size_of::<Arc<ConformingSurfaceMesh>>(),
                ),
                (face_count, std::mem::size_of::<CellFaceReport>()),
            ],
            std::mem::size_of::<CellSeamBatch>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch = size(
            &[(inputs.len(), std::mem::size_of::<u32>())],
            std::mem::size_of::<Vec<u32>>(),
        )?;
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut cells = exact(inputs.len())?;
        let mut faces = exact(face_count)?;
        let mut epochs = exact(inputs.len())?;
        for cell in inputs {
            check(&cancel)?;
            cells.push(Arc::clone(cell));
            epochs.push(0u32);
        }
        for i in 0..cells.len() {
            for j in i + 1..cells.len() {
                check(&cancel)?;
                let a = chart(&cells[i]).graph().cell();
                let b = chart(&cells[j]).graph().cell();
                if let Some((axis, upper)) = adjacent(a.origin(), b.origin()) {
                    let face = a.face(axis, upper).ok_or(SurfaceError::Geometry)?.0;
                    if b.face(axis, !upper).ok_or(SurfaceError::Geometry)?.0 != face {
                        return Err(SurfaceError::Geometry);
                    }
                    faces.push(CellFaceReport {
                        cells: [
                            u16::try_from(i).map_err(|_| SurfaceError::Limit)?,
                            u16::try_from(j).map_err(|_| SurfaceError::Limit)?,
                        ],
                        face,
                        state: CellFaceState::Unresolved {
                            reason: CellSeamStop::NotRechecked,
                        },
                        observed: [u32::MAX; 2],
                    });
                }
            }
        }
        let (mut rounds, mut visits, mut updates) = (0u8, 0usize, 0usize);
        for _ in 0..limits.max_rounds {
            check(&cancel)?;
            rounds += 1;
            let mut changed = false;
            for report in &mut faces {
                check(&cancel)?;
                let [a, b] = report.cells.map(usize::from);
                let versions = [epochs[a], epochs[b]];
                if report.observed == versions {
                    continue;
                }
                if visits == limits.max_face_visits {
                    break;
                }
                visits += 1;
                let ca = chart(&cells[a]);
                let cb = chart(&cells[b]);
                let reason = if ca.axis() != cb.axis() {
                    Some(CellSeamStop::ProjectionMismatch)
                } else if ca.axis() == report.face.axis {
                    Some(CellSeamStop::ProjectionEndFace)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    report.state = CellFaceState::Unresolved { reason };
                    report.observed = versions;
                    continue;
                }
                let partition = match cells[a].shared_face_partition_with_budget(
                    &cells[b],
                    usize::from(report.face.axis),
                    limits.partition,
                    Arc::clone(&budget),
                    &cancel,
                ) {
                    Ok(value) => value,
                    Err(e) => {
                        report.state = stage_error(
                            e,
                            CellSeamStop::PartitionGeometry,
                            CellSeamStop::PartitionLimit,
                        )?;
                        report.observed = versions;
                        continue;
                    }
                };
                let mut splits = [0usize; 2];
                for p in &partition.points {
                    check(&cancel)?;
                    for (side, placement) in p.placements.iter().enumerate() {
                        splits[side] +=
                            usize::from(matches!(placement, FacePlacement::Split { .. }));
                    }
                }
                if splits == [0, 0] {
                    report.state = face_result(&partition, &cancel)?;
                    report.observed = versions;
                    continue;
                }
                if updates == limits.max_updates {
                    report.state = CellFaceState::Unresolved {
                        reason: CellSeamStop::UpdateLimit,
                    };
                    report.observed = versions;
                    continue;
                }
                let partition = Arc::new(partition);
                let pair =
                    match partition.apply_with_budget(limits.apply, Arc::clone(&budget), &cancel) {
                        Ok(value) => value,
                        Err(e) => {
                            report.state = stage_error(
                                e,
                                CellSeamStop::ApplyGeometry,
                                CellSeamStop::ApplyLimit,
                            )?;
                            report.observed = versions;
                            continue;
                        }
                    };
                let pair = Arc::new(pair);
                let promoted = match pair.promote_cells_with_budget(
                    limits.promotion,
                    Arc::clone(&budget),
                    &cancel,
                ) {
                    Ok(value) => value,
                    Err(e) => {
                        report.state = stage_error(
                            e,
                            CellSeamStop::PromotionGeometry,
                            CellSeamStop::PromotionLimit,
                        )?;
                        report.observed = versions;
                        continue;
                    }
                };
                let [left, right] = promoted;
                // Retain the original certificates on an unchanged side; applying
                // the exchange recertifies both sides but only split sides change.
                if splits[0] != 0 {
                    cells[a] = left;
                    epochs[a] = epochs[a].checked_add(1).ok_or(SurfaceError::Limit)?;
                }
                if splits[1] != 0 {
                    cells[b] = right;
                    epochs[b] = epochs[b].checked_add(1).ok_or(SurfaceError::Limit)?;
                }
                updates += 1;
                changed = true;
                report.state = CellFaceState::Unresolved {
                    reason: CellSeamStop::NotRechecked,
                };
                report.observed = [u32::MAX; 2];
            }
            if !changed || visits == limits.max_face_visits {
                break;
            }
        }
        for report in &mut faces {
            check(&cancel)?;
            let [a, b] = report.cells.map(usize::from);
            if report.observed != [epochs[a], epochs[b]] {
                report.state = CellFaceState::Unresolved {
                    reason: CellSeamStop::NotRechecked,
                };
            }
        }
        check(&cancel)?;
        Ok(Self {
            cells,
            faces,
            rounds,
            visits,
            updates,
            bytes,
            _reservation: reservation,
        })
    }
}
