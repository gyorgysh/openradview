// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded mixed affine/candidate coordination over one immutable assembly.
use super::*;

const MAX_ROUNDS: u8 = 16;
const MAX_VISITS: usize = 16_384;
const MAX_UPDATES: usize = 1024;
const MAX_FACES: usize = 3 * MAX_CELLS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixedCoordinationLimits {
    pub max_rounds: u8,
    /// Counts mixed-face visits; general candidate coordination has its own limit.
    pub max_face_visits: usize,
    /// Maximum candidate generations adopted into this snapshot.
    pub max_candidate_updates: usize,
    /// Retained directory only; nested batches and geometries are admitted separately.
    pub max_directory_bytes: u64,
    pub general: CellSeamLimits,
    pub partition: FacePartitionLimits,
    pub apply: AppliedFaceLimits,
    pub promotion: FacePromotionLimits,
    pub affine: AffineFaceMergeLimits,
}
impl Default for MixedCoordinationLimits {
    fn default() -> Self {
        Self {
            max_rounds: MAX_ROUNDS,
            max_face_visits: MAX_VISITS,
            max_candidate_updates: MAX_UPDATES,
            max_directory_bytes: MAX_DIRECTORY,
            general: CellSeamLimits::default(),
            partition: FacePartitionLimits::default(),
            apply: AppliedFaceLimits::default(),
            promotion: FacePromotionLimits::default(),
            affine: AffineFaceMergeLimits::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedFaceStop {
    NotRechecked,
    CandidateUnavailable,
    Unsupported(MixedFaceUnsupported),
    Partition(SurfaceError),
    Apply(SurfaceError),
    ApplyRefused(MixedFaceApplyRefusal),
    Promotion(SurfaceError),
    UpdateLimit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedFaceState {
    /// The current local outputs are retained; these counts do not prove a surface.
    Applied {
        points: u32,
        spans: u32,
        missing_points: u32,
        missing_spans: u32,
        uncertified_spans: u32,
    },
    Unresolved {
        reason: MixedFaceStop,
    },
}
pub struct MixedFaceReport {
    face: FaceKey,
    affine_cell: u16,
    candidate_cell: u16,
    candidate_mesh: Option<u16>,
    state: MixedFaceState,
    observed: u32,
    application: Option<Arc<AppliedMixedFace>>,
}
impl MixedFaceReport {
    pub fn face(&self) -> FaceKey {
        self.face
    }
    /// Indices into the assembly's cell directory.
    pub fn cells(&self) -> [u16; 2] {
        [self.affine_cell, self.candidate_cell]
    }
    /// Index into the coordinator's current candidate meshes, if one exists.
    pub fn candidate_mesh(&self) -> Option<u16> {
        self.candidate_mesh
    }
    pub fn state(&self) -> MixedFaceState {
        self.state
    }
    pub fn application(&self) -> Option<&Arc<AppliedMixedFace>> {
        self.application.as_ref()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedAffineState {
    NotPlane,
    NoReadyFaces {
        listed: u8,
    },
    /// Ready faces were merged; missing or unlisted faces remain outside this result.
    Ready {
        applied: u8,
        listed: u8,
    },
    Refused(AffineFaceMergeRefusal),
    Unresolved(SurfaceError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedGeneralState {
    NoCandidates,
    /// The retained general-candidate reports correspond to the current meshes.
    Current,
    Unresolved(SurfaceError),
    NotRechecked,
}

/// A private snapshot of listed mixed faces and candidate generations. Neither
/// completed face coverage nor finite-grid closure is implied by this directory.
pub struct MixedCoordination {
    assembly: Arc<CandidateAssembly>,
    meshes: Vec<Arc<ConformingSurfaceMesh>>,
    faces: Vec<MixedFaceReport>,
    affine: Vec<Option<Arc<AffineFaceMesh>>>,
    affine_states: Vec<MixedAffineState>,
    general: Option<CellSeamBatch>,
    general_state: MixedGeneralState,
    rounds: u8,
    visits: usize,
    updates: usize,
    bytes: u64,
    _reservation: Reservation,
}
impl MixedCoordination {
    pub fn assembly(&self) -> &Arc<CandidateAssembly> {
        &self.assembly
    }
    pub fn meshes(&self) -> &[Arc<ConformingSurfaceMesh>] {
        &self.meshes
    }
    pub fn faces(&self) -> &[MixedFaceReport] {
        &self.faces
    }
    /// One entry per assembly cell, aligned with `affine_states()`.
    pub fn affine_meshes(&self) -> &[Option<Arc<AffineFaceMesh>>] {
        &self.affine
    }
    pub fn affine_states(&self) -> &[MixedAffineState] {
        &self.affine_states
    }
    pub fn general_state(&self) -> MixedGeneralState {
        self.general_state
    }
    pub fn general_faces(&self) -> &[CellFaceReport] {
        self.general.as_ref().map_or(&[], CellSeamBatch::faces)
    }
    pub fn rounds(&self) -> u8 {
        self.rounds
    }
    pub fn face_visits(&self) -> usize {
        self.visits
    }
    pub fn candidate_updates(&self) -> usize {
        self.updates
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }

    /// Coordinate listed candidates and mixed faces until stable or bounded.
    /// Worker-only; stale, missing, unsupported and uncertified faces stay explicit.
    pub fn coordinate(
        assembly: &Arc<CandidateAssembly>,
        limits: MixedCoordinationLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        Self::coordinate_with_budget(assembly, limits, MemoryBudget::volumes(), cancel)
    }

    pub(in crate::surface) fn coordinate_with_budget(
        assembly: &Arc<CandidateAssembly>,
        limits: MixedCoordinationLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        if limits.max_rounds == 0
            || limits.max_rounds > MAX_ROUNDS
            || limits.max_face_visits > MAX_VISITS
            || limits.max_candidate_updates > MAX_UPDATES
            || limits.max_directory_bytes > MAX_DIRECTORY
            || assembly.cells().len() > MAX_CELLS
            || assembly.affine_faces().len() > MAX_FACES
        {
            return Err(SurfaceError::Limit);
        }
        let count = assembly
            .affine_faces()
            .iter()
            .try_fold(0usize, |count, face| {
                if cancel() {
                    return Err(SurfaceError::Cancelled);
                }
                let [a, b] = face_cells(assembly, face.key())?;
                let planes = [a, b]
                    .map(|i| assembly.affine_cells()[i].disposition() == AffineDisposition::Plane);
                count
                    .checked_add(usize::from(planes[0] != planes[1]))
                    .ok_or(SurfaceError::Limit)
            })?;
        let bytes = directory_size(
            &[
                (
                    assembly.meshes().len(),
                    std::mem::size_of::<Arc<ConformingSurfaceMesh>>(),
                ),
                (count, std::mem::size_of::<MixedFaceReport>()),
                (
                    assembly.cells().len(),
                    std::mem::size_of::<Option<Arc<AffineFaceMesh>>>(),
                ),
                (
                    assembly.cells().len(),
                    std::mem::size_of::<MixedAffineState>(),
                ),
            ],
            std::mem::size_of::<Self>() + crate::surface::shared_header_bytes::<Self>(),
        )?;
        if bytes > limits.max_directory_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch = directory_size(
            &[
                (assembly.meshes().len(), std::mem::size_of::<u32>()),
                (6, std::mem::size_of::<Arc<AppliedMixedFace>>()),
            ],
            2 * std::mem::size_of::<Vec<u32>>(),
        )?;
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut meshes = exact(assembly.meshes().len())?;
        let mut epochs = exact(assembly.meshes().len())?;
        let mut faces = exact(count)?;
        let mut affine = exact(assembly.cells().len())?;
        let mut affine_states = exact(assembly.cells().len())?;
        let mut group = exact(6)?;
        for mesh in assembly.meshes() {
            meshes.push(Arc::clone(mesh));
            epochs.push(0u32);
        }
        for face in assembly.affine_faces() {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            let [a, b] = face_cells(assembly, face.key())?;
            let planes = [a, b]
                .map(|i| assembly.affine_cells()[i].disposition() == AffineDisposition::Plane);
            let (plane, candidate) = match planes {
                [true, false] => (a, b),
                [false, true] => (b, a),
                _ => continue,
            };
            let candidate_mesh = match assembly.cells()[candidate].state {
                CandidateCellState::Candidate { mesh } => Some(mesh),
                _ => None,
            };
            faces.push(MixedFaceReport {
                face: face.key(),
                affine_cell: u16::try_from(plane).map_err(|_| SurfaceError::Limit)?,
                candidate_cell: u16::try_from(candidate).map_err(|_| SurfaceError::Limit)?,
                candidate_mesh,
                state: MixedFaceState::Unresolved {
                    reason: if candidate_mesh.is_some() {
                        MixedFaceStop::NotRechecked
                    } else {
                        MixedFaceStop::CandidateUnavailable
                    },
                },
                observed: u32::MAX,
                application: None,
            });
        }
        if faces.len() != count {
            return Err(SurfaceError::Geometry);
        }
        let mut general = None;
        let mut general_state = if meshes.is_empty() {
            MixedGeneralState::NoCandidates
        } else {
            MixedGeneralState::NotRechecked
        };
        let (mut rounds, mut visits, mut updates) = (0u8, 0usize, 0usize);
        for _ in 0..limits.max_rounds {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            rounds += 1;
            if !meshes.is_empty() {
                match CellSeamBatch::coordinate_with_budget(
                    &meshes,
                    limits.general,
                    Arc::clone(&budget),
                    &cancel,
                ) {
                    Ok(batch) => {
                        if batch.cells().len() != meshes.len() {
                            return Err(SurfaceError::Geometry);
                        }
                        let changed = meshes
                            .iter()
                            .zip(batch.cells())
                            .filter(|(a, b)| !Arc::ptr_eq(a, b))
                            .count();
                        if updates.checked_add(changed).ok_or(SurfaceError::Limit)?
                            > limits.max_candidate_updates
                        {
                            general = None;
                            general_state = MixedGeneralState::Unresolved(SurfaceError::Limit);
                        } else {
                            for (index, next) in batch.cells().iter().enumerate() {
                                if !Arc::ptr_eq(&meshes[index], next) {
                                    meshes[index] = Arc::clone(next);
                                    epochs[index] =
                                        epochs[index].checked_add(1).ok_or(SurfaceError::Limit)?;
                                }
                            }
                            updates += changed;
                            general = Some(batch);
                            general_state = MixedGeneralState::Current;
                        }
                    }
                    Err(SurfaceError::Limit) => {
                        general = None;
                        general_state = MixedGeneralState::Unresolved(SurfaceError::Limit);
                    }
                    Err(SurfaceError::Geometry) => {
                        general = None;
                        general_state = MixedGeneralState::Unresolved(SurfaceError::Geometry);
                    }
                    Err(error) => return Err(error),
                }
            }
            let mut changed = false;
            for report in &mut faces {
                if cancel() {
                    return Err(SurfaceError::Cancelled);
                }
                let Some(mesh_id) = report.candidate_mesh.map(usize::from) else {
                    continue;
                };
                let epoch = *epochs.get(mesh_id).ok_or(SurfaceError::Geometry)?;
                if report.observed == epoch {
                    continue;
                }
                report.application = None;
                report.state = MixedFaceState::Unresolved {
                    reason: MixedFaceStop::NotRechecked,
                };
                if visits == limits.max_face_visits {
                    break;
                }
                visits += 1;
                let mesh = meshes.get(mesh_id).ok_or(SurfaceError::Geometry)?;
                let plane = assembly
                    .affine_cells()
                    .get(usize::from(report.affine_cell))
                    .ok_or(SurfaceError::Geometry)?;
                let partition = match mesh.mixed_face_partition_with_budget(
                    plane,
                    usize::from(report.face.axis),
                    limits.partition,
                    Arc::clone(&budget),
                    &cancel,
                ) {
                    Ok(Ok(partition)) => partition,
                    Ok(Err(reason)) => {
                        report.state = MixedFaceState::Unresolved {
                            reason: MixedFaceStop::Unsupported(reason),
                        };
                        report.observed = epoch;
                        continue;
                    }
                    Err(error) => {
                        report.state = face_error(error, MixedFaceStop::Partition)?;
                        report.observed = epoch;
                        continue;
                    }
                };
                let partition = Arc::new(partition);
                let applied = match partition.apply_mixed_with_budget(
                    assembly.max_error_mm(),
                    limits.apply,
                    Arc::clone(&budget),
                    &cancel,
                ) {
                    Ok(Ok(value)) => value,
                    Ok(Err(reason)) => {
                        report.state = MixedFaceState::Unresolved {
                            reason: MixedFaceStop::ApplyRefused(reason),
                        };
                        report.observed = epoch;
                        continue;
                    }
                    Err(error) => {
                        report.state = face_error(error, MixedFaceStop::Apply)?;
                        report.observed = epoch;
                        continue;
                    }
                };
                if !candidate_unchanged(mesh, &applied, limits.apply.tolerance_bits[0], &cancel)? {
                    if updates == limits.max_candidate_updates {
                        report.state = MixedFaceState::Unresolved {
                            reason: MixedFaceStop::UpdateLimit,
                        };
                        report.observed = epoch;
                        continue;
                    }
                    let promoted = match applied.promote_candidate_compacted_with_budget(
                        limits.promotion,
                        Arc::clone(&budget),
                        &cancel,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            report.state = face_error(error, MixedFaceStop::Promotion)?;
                            report.observed = epoch;
                            continue;
                        }
                    };
                    meshes[mesh_id] = promoted;
                    epochs[mesh_id] = epoch.checked_add(1).ok_or(SurfaceError::Limit)?;
                    updates += 1;
                    changed = true;
                    report.observed = u32::MAX;
                    general = None;
                    general_state = MixedGeneralState::NotRechecked;
                    continue;
                }
                report.state = face_state(&applied, &cancel)?;
                report.observed = epoch;
                report.application = Some(Arc::new(applied));
            }
            if !changed {
                break;
            }
        }
        for report in &mut faces {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            if let Some(mesh) = report.candidate_mesh.map(usize::from)
                && report.observed != *epochs.get(mesh).ok_or(SurfaceError::Geometry)?
            {
                report.state = MixedFaceState::Unresolved {
                    reason: MixedFaceStop::NotRechecked,
                };
                report.application = None;
            }
        }
        for (cell_id, plane) in assembly.affine_cells().iter().enumerate() {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            if plane.disposition() != AffineDisposition::Plane {
                affine.push(None);
                affine_states.push(MixedAffineState::NotPlane);
                continue;
            }
            group.clear();
            let mut listed = 0u8;
            for face in &faces {
                if cancel() {
                    return Err(SurfaceError::Cancelled);
                }
                if usize::from(face.affine_cell) == cell_id {
                    listed = listed.checked_add(1).ok_or(SurfaceError::Limit)?;
                    if let Some(application) = &face.application {
                        if group.len() == group.capacity() {
                            return Err(SurfaceError::Limit);
                        }
                        group.push(Arc::clone(application));
                    }
                }
            }
            if group.is_empty() {
                affine.push(None);
                affine_states.push(MixedAffineState::NoReadyFaces { listed });
                continue;
            }
            match AffineFaceMesh::merge_with_budget(
                &group,
                assembly.max_error_mm(),
                limits.affine,
                Arc::clone(&budget),
                &cancel,
            ) {
                Ok(Ok(mesh)) => {
                    affine.push(Some(Arc::new(mesh)));
                    affine_states.push(MixedAffineState::Ready {
                        applied: u8::try_from(group.len()).map_err(|_| SurfaceError::Limit)?,
                        listed,
                    });
                }
                Ok(Err(reason)) => {
                    affine.push(None);
                    affine_states.push(MixedAffineState::Refused(reason));
                }
                Err(SurfaceError::Limit) => {
                    affine.push(None);
                    affine_states.push(MixedAffineState::Unresolved(SurfaceError::Limit));
                }
                Err(SurfaceError::Geometry) => {
                    affine.push(None);
                    affine_states.push(MixedAffineState::Unresolved(SurfaceError::Geometry));
                }
                Err(error) => return Err(error),
            }
        }
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        Ok(Self {
            assembly: Arc::clone(assembly),
            meshes,
            faces,
            affine,
            affine_states,
            general,
            general_state,
            rounds,
            visits,
            updates,
            bytes,
            _reservation: reservation,
        })
    }
}

fn face_cells(assembly: &CandidateAssembly, face: FaceKey) -> Result<[usize; 2], SurfaceError> {
    let axis = usize::from(face.axis);
    if axis >= 3 {
        return Err(SurfaceError::Geometry);
    }
    let mut negative = face.origin;
    negative[axis] = negative[axis]
        .checked_sub(1)
        .ok_or(SurfaceError::Geometry)?;
    let find = |origin: [u32; 3]| {
        assembly
            .cells()
            .binary_search_by_key(
                &(origin[2] as usize, origin[1] as usize, origin[0] as usize),
                |cell| (cell.origin[2], cell.origin[1], cell.origin[0]),
            )
            .map_err(|_| SurfaceError::Geometry)
    };
    Ok([find(negative)?, find(face.origin)?])
}
fn directory_size(parts: &[(usize, usize)], base: usize) -> Result<u64, SurfaceError> {
    parts
        .iter()
        .try_fold(base, |total, (count, size)| {
            count
                .checked_mul(*size)
                .and_then(|part| total.checked_add(part))
                .ok_or(SurfaceError::Limit)
        })
        .and_then(|value| u64::try_from(value).map_err(|_| SurfaceError::Limit))
}
fn face_error(
    error: SurfaceError,
    stage: fn(SurfaceError) -> MixedFaceStop,
) -> Result<MixedFaceState, SurfaceError> {
    match error {
        SurfaceError::Limit | SurfaceError::Geometry => Ok(MixedFaceState::Unresolved {
            reason: stage(error),
        }),
        other => Err(other),
    }
}
fn candidate_unchanged(
    source: &ConformingSurfaceMesh,
    applied: &AppliedMixedFace,
    bits: u8,
    cancel: &impl Fn() -> bool,
) -> Result<bool, SurfaceError> {
    let output = applied.candidate();
    if source
        .axial_tolerance_bits()
        .is_some_and(|current| current != bits)
        || source.vertices().len() != output.vertices().len()
        || source.triangles().len() != output.triangles().len()
    {
        return Ok(false);
    }
    for (index, (source, output)) in source.vertices().iter().zip(output.vertices()).enumerate() {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        if !matches!(output, AppliedFaceVertex::Retained { id, position } if *id as usize == index && *position == source.position())
        {
            return Ok(false);
        }
    }
    for (index, (source, output)) in source
        .triangles()
        .iter()
        .zip(output.triangles())
        .enumerate()
    {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        if output.source_triangle as usize != index
            || output.vertices != source.vertices
            || output.source_facet != source.source_facet
        {
            return Ok(false);
        }
        let same = match (source.state, output.state) {
            (
                ConformingFacetState::WithinTolerance {
                    numerator: a,
                    denominator: b,
                },
                AppliedTriangleState::WithinTolerance {
                    numerator: c,
                    denominator: d,
                },
            ) => a == c && b == d,
            (
                ConformingFacetState::Unresolved {
                    certificate: Some(a),
                    reason: ConformingStopReason::FaceRecertification,
                },
                AppliedTriangleState::Unresolved { certificate: b },
            ) => a == b,
            _ => false,
        };
        if !same {
            return Ok(false);
        }
    }
    Ok(true)
}
fn face_state(
    applied: &AppliedMixedFace,
    cancel: &impl Fn() -> bool,
) -> Result<MixedFaceState, SurfaceError> {
    let mut missing_points = 0u32;
    let mut missing_spans = 0u32;
    let mut uncertified_spans = 0u32;
    for point in applied.point_vertices() {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        missing_points = missing_points
            .checked_add(u32::from(point.contains(&None)))
            .ok_or(SurfaceError::Limit)?;
    }
    for span in applied.spans() {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        missing_spans = missing_spans
            .checked_add(u32::from(span.segments.contains(&None)))
            .ok_or(SurfaceError::Limit)?;
        uncertified_spans = uncertified_spans
            .checked_add(u32::from(
                span.segments[0].is_some() && !span.candidate_within_tolerance,
            ))
            .ok_or(SurfaceError::Limit)?;
    }
    Ok(MixedFaceState::Applied {
        points: u32::try_from(applied.point_vertices().len()).map_err(|_| SurfaceError::Limit)?,
        spans: u32::try_from(applied.spans().len()).map_err(|_| SurfaceError::Limit)?,
        missing_points,
        missing_spans,
        uncertified_spans,
    })
}
