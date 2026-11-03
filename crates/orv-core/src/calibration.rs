// SPDX-License-Identifier: GPL-3.0-or-later
//! Ultrasound region calibration (PS3.3 C.8.5.5). Physical spacing applies only
//! inside one declared two-dimensional region with centimetre units. Points that
//! leave a region, cross regions or lie where regions overlap stay uncalibrated
//! instead of borrowing another region's scale.
use dicom_core::Tag;
use dicom_object::InMemDicomObject;
use std::sync::Arc;

pub const MAX_REGIONS: usize = 64;
const SEQUENCE: Tag = Tag(0x18, 0x6011);
/// Region Spatial Format 1 is a 2D image; Physical Units 3 is centimetres.
const SPATIAL_2D: u16 = 1;
const CENTIMETRES: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Inclusive zero-based pixel bounds: [x0, y0] and [x1, y1].
    pub min: [u32; 2],
    pub max: [u32; 2],
    /// Millimetres per pixel along x (columns) and y (rows).
    pub spacing_mm: [f64; 2],
}
impl Region {
    fn contains(&self, pixel: [f64; 2]) -> bool {
        (0..2).all(|axis| {
            pixel[axis].is_finite()
                && pixel[axis] >= f64::from(self.min[axis]) - 0.5
                && pixel[axis] <= f64::from(self.max[axis]) + 0.5
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    regions: Vec<Region>,
}
impl Calibration {
    /// Validated regions within a `width` x `height` grid; invalid ones are dropped.
    pub fn new(regions: Vec<Region>, width: u32, height: u32) -> Option<Self> {
        let regions: Vec<Region> = regions
            .into_iter()
            .take(MAX_REGIONS)
            .filter(|r| {
                r.min[0] <= r.max[0]
                    && r.min[1] <= r.max[1]
                    && r.max[0] < width
                    && r.max[1] < height
                    && r.spacing_mm.iter().all(|v| v.is_finite() && *v > 0.)
            })
            .collect();
        (!regions.is_empty()).then_some(Self { regions })
    }
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }
    /// The single calibrated region containing every point, if there is one.
    pub fn region_for(&self, points: &[[f64; 2]]) -> Option<&Region> {
        let mut found = None;
        for region in &self.regions {
            if points.iter().all(|p| region.contains(*p)) {
                if found.is_some() {
                    return None;
                }
                found = Some(region);
            }
        }
        // A point also inside a differently scaled region is ambiguous.
        let region = found?;
        let ambiguous = points.iter().any(|p| {
            self.regions
                .iter()
                .any(|other| other != region && other.contains(*p))
        });
        (!ambiguous).then_some(region)
    }
    /// Points in millimetres within their shared region.
    pub fn millimetres(&self, points: &[[f64; 2]]) -> Option<Vec<[f64; 3]>> {
        let region = self.region_for(points)?;
        Some(
            points
                .iter()
                .map(|p| [p[0] * region.spacing_mm[0], p[1] * region.spacing_mm[1], 0.])
                .collect(),
        )
    }
}

fn number<T: Copy>(
    item: &InMemDicomObject,
    tag: Tag,
    read: impl Fn(&dicom_object::mem::InMemElement) -> Option<T>,
) -> Option<T> {
    item.element(tag).ok().and_then(read)
}

/// Keep only complete, finite, positive centimetre regions; malformed or other
/// regions are skipped rather than repaired.
pub fn read(object: &InMemDicomObject, width: u32, height: u32) -> Option<Arc<Calibration>> {
    let items = object.element(SEQUENCE).ok()?.value().items()?;
    if items.len() > MAX_REGIONS {
        return None;
    }
    let short = |e: &dicom_object::mem::InMemElement| e.to_int::<u16>().ok();
    let long = |e: &dicom_object::mem::InMemElement| e.to_int::<u32>().ok();
    let float = |e: &dicom_object::mem::InMemElement| {
        e.to_float64().ok().filter(|v| v.is_finite() && *v > 0.)
    };
    let regions: Vec<Region> = items
        .iter()
        .filter_map(|item| {
            if number(item, Tag(0x18, 0x6012), short)? != SPATIAL_2D
                || number(item, Tag(0x18, 0x6024), short)? != CENTIMETRES
                || number(item, Tag(0x18, 0x6026), short)? != CENTIMETRES
            {
                return None;
            }
            let min = [
                number(item, Tag(0x18, 0x6018), long)?,
                number(item, Tag(0x18, 0x601a), long)?,
            ];
            let max = [
                number(item, Tag(0x18, 0x601c), long)?,
                number(item, Tag(0x18, 0x601e), long)?,
            ];
            let spacing_mm = [
                number(item, Tag(0x18, 0x602c), float)? * 10.,
                number(item, Tag(0x18, 0x602e), float)? * 10.,
            ];
            Some(Region {
                min,
                max,
                spacing_mm,
            })
        })
        .collect();
    Calibration::new(regions, width, height).map(Arc::new)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, VR};

    pub(crate) fn region(bounds: [u32; 4], delta_cm: [f64; 2], units: u16) -> InMemDicomObject {
        let us = |tag, v: u16| DataElement::new(tag, VR::US, PrimitiveValue::from(v));
        let ul = |tag, v: u32| DataElement::new(tag, VR::UL, PrimitiveValue::from(v));
        let fd = |tag, v: f64| DataElement::new(tag, VR::FD, PrimitiveValue::from(v));
        InMemDicomObject::from_element_iter([
            us(Tag(0x18, 0x6012), SPATIAL_2D),
            ul(Tag(0x18, 0x6018), bounds[0]),
            ul(Tag(0x18, 0x601a), bounds[1]),
            ul(Tag(0x18, 0x601c), bounds[2]),
            ul(Tag(0x18, 0x601e), bounds[3]),
            us(Tag(0x18, 0x6024), units),
            us(Tag(0x18, 0x6026), units),
            fd(Tag(0x18, 0x602c), delta_cm[0]),
            fd(Tag(0x18, 0x602e), delta_cm[1]),
        ])
    }
    pub(crate) fn object(regions: Vec<InMemDicomObject>) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([DataElement::new(
            SEQUENCE,
            VR::SQ,
            DicomValue::new_sequence(regions, Length::UNDEFINED),
        )])
    }

    #[test]
    fn distances_use_one_region_and_refuse_crossing_outside_or_overlapping_points() {
        let calibration = read(
            &object(vec![
                region([0, 0, 49, 99], [0.01, 0.02], CENTIMETRES),
                region([60, 0, 99, 99], [0.05, 0.05], CENTIMETRES),
                // Seconds per pixel: not spatial, skipped.
                region([50, 0, 59, 99], [0.1, 0.1], 4),
            ]),
            100,
            100,
        )
        .unwrap();
        assert_eq!(calibration.regions().len(), 2);
        let mm = calibration.millimetres(&[[0., 0.], [30., 40.]]).unwrap();
        let length = crate::measurements::distance_mm(mm[0], mm[1]).unwrap();
        assert!((length - (3f64.powi(2) + 8f64.powi(2)).sqrt()).abs() < 1e-9);
        let far = calibration.millimetres(&[[60., 0.], [99., 0.]]).unwrap();
        assert!((crate::measurements::distance_mm(far[0], far[1]).unwrap() - 19.5).abs() < 1e-9);
        assert!(calibration.millimetres(&[[10., 10.], [70., 10.]]).is_none());
        assert!(calibration.millimetres(&[[55., 10.], [40., 10.]]).is_none());
        let overlapping = read(
            &object(vec![
                region([0, 0, 49, 49], [0.01, 0.01], CENTIMETRES),
                region([40, 40, 99, 99], [0.02, 0.02], CENTIMETRES),
            ]),
            100,
            100,
        )
        .unwrap();
        assert!(overlapping.millimetres(&[[10., 10.], [20., 20.]]).is_some());
        assert!(overlapping.millimetres(&[[10., 10.], [45., 45.]]).is_none());
    }

    #[test]
    fn incomplete_out_of_image_or_nonpositive_regions_are_skipped() {
        for (bounds, delta) in [
            ([0, 0, 100, 10], [0.01, 0.01]),
            ([20, 0, 10, 10], [0.01, 0.01]),
            ([0, 0, 10, 10], [0., 0.01]),
            ([0, 0, 10, 10], [f64::NAN, 0.01]),
        ] {
            assert!(read(&object(vec![region(bounds, delta, CENTIMETRES)]), 100, 100).is_none());
        }
        let mut partial = region([0, 0, 10, 10], [0.01, 0.01], CENTIMETRES);
        partial.remove_element(Tag(0x18, 0x602e));
        assert!(read(&object(vec![partial]), 100, 100).is_none());
        assert!(read(&InMemDicomObject::new_empty(), 100, 100).is_none());
    }
}
