// SPDX-License-Identifier: GPL-3.0-or-later
//! Geometry-shared color resampling; linear-light presentation, separate RGB codes.
use super::*;
use crate::color::{ColorData, ColorInfo};
impl Volume {
    pub fn is_color(&self) -> bool {
        self.frames[0].rgba.is_some()
    }
    pub fn color_info(&self) -> Option<&ColorInfo> {
        self.frames[0].color.as_ref().map(|data| &data.info)
    }
    /// Linear-light sRGB, not modality intensity. Missing samples remain missing.
    pub fn sample_rgb(&self, world: V3) -> Option<[f32; 3]> {
        self.color_sample(world)
            .map(|(linear, _)| linear.map(|v| v as f32))
    }
    fn color_sample(&self, world: V3) -> Option<([f64; 3], [f64; 3])> {
        self.color_sample_voxel(self.world_to_voxel(world))
    }
    fn color_sample_voxel(&self, v: V3) -> Option<([f64; 3], [f64; 3])> {
        if !self.is_color() {
            return None;
        }
        if !finite(v)
            || v.iter()
                .enumerate()
                .any(|(i, p)| *p < -1e-6 || *p > (self.dimensions[i] - 1) as f64 + 1e-6)
        {
            return None;
        }
        let v: V3 = std::array::from_fn(|i| v[i].clamp(0., (self.dimensions[i] - 1) as f64));
        let lo = v.map(|p| p.floor() as usize);
        let hi: [usize; 3] = std::array::from_fn(|i| (lo[i] + 1).min(self.dimensions[i] - 1));
        let t: V3 = std::array::from_fn(|i| v[i] - lo[i] as f64);
        let (mut linear, mut codes) = ([0.; 3], [0.; 3]);
        for (z, wz) in [(lo[2], 1. - t[2]), (hi[2], t[2])] {
            for (y, wy) in [(lo[1], 1. - t[1]), (hi[1], t[1])] {
                for (x, wx) in [(lo[0], 1. - t[0]), (hi[0], t[0])] {
                    let weight = wx * wy * wz;
                    if weight <= 1e-12 {
                        continue;
                    }
                    let frame = &self.frames[z];
                    let i = y * self.dimensions[0] + x;
                    if !frame.has_sample(i) {
                        return None;
                    }
                    let rgba = frame.rgba.as_ref()?;
                    for c in 0..3 {
                        linear[c] += linear_code(rgba[i * 4 + c]) * weight;
                        codes[c] += frame
                            .color
                            .as_ref()
                            .map_or(rgba[i * 4 + c] as f64, |data| data.rgb[i * 3 + c] as f64)
                            * weight;
                    }
                }
            }
        }
        Some((linear, codes))
    }
    pub(super) fn resample_color(
        &self,
        geometry: Geometry,
        dimensions: [u32; 2],
        cancel: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, GeometryError> {
        let mut rgba = Vec::with_capacity(dimensions[0] as usize * dimensions[1] as usize * 4);
        let mut codes = self
            .color_info()
            .map(|_| Vec::with_capacity(dimensions[0] as usize * dimensions[1] as usize * 3));
        let (u, v) = geometry.directions();
        let origin = self.world_to_voxel(geometry.position);
        let du = self.world_vector_to_voxel(mul(u, geometry.spacing[1]));
        let dv = self.world_vector_to_voxel(mul(v, geometry.spacing[0]));
        for y in 0..dimensions[1] {
            if cancel() {
                return Err(GeometryError::Cancelled);
            }
            let row = add(origin, mul(dv, y as f64));
            for x in 0..dimensions[0] {
                if let Some((linear, rgb)) = self.color_sample_voxel(add(row, mul(du, x as f64))) {
                    rgba.extend(linear.map(encoded_code));
                    rgba.push(255);
                    if let Some(codes) = &mut codes {
                        codes.extend(rgb.map(|v| {
                            v.round()
                                .clamp(0., self.color_info().map_or(255, ColorInfo::maximum) as f64)
                                as u16
                        }));
                    }
                } else {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                    if let Some(codes) = &mut codes {
                        codes.extend_from_slice(&[0; 3]);
                    }
                }
            }
        }
        let color = codes.zip(self.color_info()).map(|(rgb, info)| {
            Arc::new(ColorData {
                info: info.clone(),
                rgb: rgb.into(),
            })
        });
        Ok(Arc::new(Frame {
            slab: None,
            scalar: None,
            validity: None,
            color,
            nifti: self.frames[0].nifti.clone(),
            width: dimensions[0],
            height: dimensions[1],
            pixels: Arc::from([]),
            rgba: Some(rgba.into()),
            geometry: Some(geometry),
            instance: 0,
            frame_index: 0,
            monochrome1: false,
            default_window: self.frames[0].default_window,
        }))
    }
}
pub fn linear_code(value: u8) -> f64 {
    static TABLE: std::sync::OnceLock<[f64; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let v = i as f64 / 255.;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        })
    })[value as usize]
}
pub fn encoded_code(linear: f64) -> u8 {
    let v = linear.clamp(0., 1.);
    let v = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    };
    (v * 255.).round().clamp(0., 255.) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WindowLevel,
        color::{ColorData, ColorEncoding, ColorInfo},
        measurements::{color_roi_stats, roi_stats},
    };
    fn volume() -> Volume {
        Volume::new(
            (0..2)
                .map(|z| {
                    Arc::new(Frame {
                        slab: None,
                        scalar: None,
                        validity: None,
                        color: Some(Arc::new(ColorData {
                            info: ColorInfo {
                                bits: 16,
                                source_encoding: ColorEncoding::Rgb,
                                icc_sha256: None,
                            },
                            rgb: (0..2)
                                .flat_map(|y| {
                                    (0..2).flat_map(move |x| [x * 65535, y * 65535, z * 65535])
                                })
                                .collect::<Vec<u16>>()
                                .into(),
                        })),
                        nifti: None,
                        width: 2,
                        height: 2,
                        pixels: Arc::from([]),
                        rgba: Some(
                            (0..2)
                                .flat_map(|y| {
                                    (0..2).flat_map(move |x| [x * 255, y * 255, z as u8 * 255, 255])
                                })
                                .collect::<Vec<u8>>()
                                .into(),
                        ),
                        geometry: Some(Geometry {
                            nifti_transform: None,
                            frame_of_reference: Some("generated-color".into()),
                            position: [0., 0., z as f64],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [1., 1.],
                        }),
                        instance: z as i32,
                        frame_index: z as u32,
                        monochrome1: false,
                        default_window: WindowLevel {
                            function: Default::default(),
                            width: 256.,
                            center: 128.,
                        },
                    })
                })
                .collect(),
        )
        .unwrap()
    }
    #[test]
    fn color_affine_sampling_is_linear_light_and_preserves_separate_code_statistics() {
        let volume = volume();
        assert!(volume.is_color());
        assert_eq!(volume.sample([0.5; 3]), None);
        assert_eq!(volume.sample_rgb([0.5; 3]), Some([0.5; 3]));
        let plane = volume
            .reformat(Plane::Axial, [0.5; 3], 32, || false)
            .unwrap();
        assert!(plane.valid_dimensions());
        assert!(plane.pixels.is_empty());
        assert_eq!(plane.rgba.as_ref().unwrap()[2], 188);
        assert_eq!(plane.color.as_ref().unwrap().rgb[2], 32768);
        let stats = color_roi_stats(&plane, [[0., 0.], [1., 1.]]).unwrap();
        assert_eq!(stats.channels[0].mean, 32767.5);
        assert_eq!(stats.channels[2].mean, 32768.);
        assert!(roi_stats(&plane, [[0., 0.], [1., 1.]]).is_err());
        let oblique = crate::oblique::ObliquePlane::from_angles([0.5; 3], [45., 30., 0.]).unwrap();
        let frame = volume.reformat_oblique(&oblique, 32, || false).unwrap();
        let opaque = frame
            .rgba
            .as_ref()
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] == 255)
            .count();
        assert!(opaque > 0 && opaque < (frame.width * frame.height) as usize);
        assert!(
            volume
                .reformat(Plane::Axial, [0.5; 3], 32, || true)
                .is_err()
        );
        assert_eq!(volume.sample_rgb([2.; 3]), None);
    }
    #[test]
    fn color_volume_refuses_mixed_scalar_profile_and_reference_context() {
        let volume = volume();
        let mut frames = volume.frames.clone();
        let mut second = (*frames[1]).clone();
        second.rgba = None;
        second.color = None;
        second.pixels = Arc::from([0f32; 4]);
        frames[1] = Arc::new(second);
        assert!(Volume::new(frames).is_err());
        let mut frames = volume.frames.clone();
        let mut second = (*frames[1]).clone();
        let mut codes = (**second.color.as_ref().unwrap()).clone();
        codes.info.icc_sha256 = Some("f".repeat(64));
        second.color = Some(Arc::new(codes));
        frames[1] = Arc::new(second);
        assert!(Volume::new(frames).is_err());
        let mut frames = volume.frames.clone();
        let mut second = (*frames[1]).clone();
        second.geometry.as_mut().unwrap().frame_of_reference =
            Some("another-generated-reference".into());
        frames[1] = Arc::new(second);
        assert!(Volume::new(frames).is_err());
    }
}
