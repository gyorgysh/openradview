// SPDX-License-Identifier: GPL-3.0-or-later
//! Incident inward fields and selected-phase classification on a zero face.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroFacePhase {
    /// Both incident neighborhoods are selected; no internal face is published.
    Interior,
    /// Outward source-lattice normal sign along the face's global normal axis.
    Interface { normal: i8 },
    /// The face is selected but both inward neighborhoods are unselected.
    /// This is a two-sided zero-thickness stratum, not a filled source volume.
    Sheet,
    /// A nonconstant inward field vanishes here; retain its curve/contact.
    Constraint,
    /// At least one incident cell is unavailable or outside. No cap is inferred.
    Unresolved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroFaceSample {
    /// Negative-normal cell first; zero is distinct from an unavailable None.
    pub inward_signs: [Option<i8>; 2],
    pub phase: ZeroFacePhase,
}
/// Exact source-owned face analysis, not a published surface or edit authority.
pub struct WholeZeroFace {
    field: MaskField,
    key: FaceKey,
    cells: [NeighborhoodCell; 2],
    normals: [Option<Bilinear>; 2],
    intersections: Option<BilinearIntersections>,
}
impl WholeZeroFace {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn key(&self) -> FaceKey {
        self.key
    }
    pub fn cells(&self) -> &[NeighborhoodCell; 2] {
        &self.cells
    }
    /// F(s,u,v)=s*G(u,v), with s directed inward from the shared face.
    pub fn inward_fields(&self) -> &[Option<Bilinear>; 2] {
        &self.normals
    }
    /// None means incomplete incident stencils, not an empty intersection set.
    pub fn intersections(&self) -> Option<&BilinearIntersections> {
        self.intersections.as_ref()
    }
    /// Pointwise exact phase away from nonconstant zero curves. Region extraction
    /// must still retain the complete two-field arrangement and shared constraints.
    pub fn sample(
        &self,
        point: [ExactCoordinate; 2],
        cancel: impl Fn() -> bool,
    ) -> Result<ZeroFaceSample, FieldError> {
        check_cancel(&cancel)?;
        let mut inward_signs = [None; 2];
        let mut constraint = false;
        for (i, field) in self.normals.iter().enumerate() {
            check_cancel(&cancel)?;
            if let Some(field) = field {
                let sign = field.sign_at(point)?;
                inward_signs[i] = Some(sign);
                constraint |= sign == 0 && field.values() != [0; 4];
            }
        }
        let phase = if inward_signs.contains(&None) {
            ZeroFacePhase::Unresolved
        } else if constraint {
            ZeroFacePhase::Constraint
        } else {
            match inward_signs.map(|s| s.is_some_and(|s| s >= 0)) {
                [true, true] => ZeroFacePhase::Interior,
                [true, false] => ZeroFacePhase::Interface { normal: 1 },
                [false, true] => ZeroFacePhase::Interface { normal: -1 },
                [false, false] => ZeroFacePhase::Sheet,
            }
        };
        check_cancel(&cancel)?;
        Ok(ZeroFaceSample {
            inward_signs,
            phase,
        })
    }
}
impl SourceFace {
    /// Analyse only a proved whole-zero face. None means the face is not proved
    /// whole zero; inspect original samples to distinguish unknown from nonzero.
    /// Outside cells remain outside until a separate finite-grid policy applies.
    pub fn whole_zero_face(
        &self,
        cancel: impl Fn() -> bool,
    ) -> Result<Option<WholeZeroFace>, FieldError> {
        check_cancel(&cancel)?;
        if !self
            .contour
            .as_ref()
            .is_some_and(FaceArrangement::is_whole_zero)
        {
            return Ok(None);
        }
        let axis = usize::from(self.key.axis);
        let mut normals = [None; 2];
        for (i, cell) in self.cells.iter().enumerate() {
            check_cancel(&cancel)?;
            if let Some(field) = cell.interpolant {
                if field.face(axis, i == 0).ok_or(FieldError::Bounds)?.values() != [0; 4] {
                    return Err(FieldError::Arithmetic);
                }
                normals[i] = field.face(axis, i != 0);
            }
        }
        let intersections = match normals {
            [Some(a), Some(b)] => Some(a.intersections(b, &cancel)?),
            _ => None,
        };
        check_cancel(&cancel)?;
        Ok(Some(WholeZeroFace {
            field: self.field.clone(),
            key: self.key,
            cells: self.cells,
            normals,
            intersections,
        }))
    }
}
