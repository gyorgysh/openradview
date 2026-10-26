// SPDX-License-Identifier: GPL-3.0-or-later
//! Pixel-centre ↔ normalized viewport mapping matching the scalar display shader.
use crate::ViewState;

pub struct ImageMapping {
    size: [u32; 2],
    scale: [f64; 2],
    pan: [f64; 2],
}
impl ImageMapping {
    pub fn new(
        size: [u32; 2],
        image_aspect: f64,
        viewport_aspect: f64,
        view: &ViewState,
    ) -> Option<Self> {
        if size.contains(&0)
            || !image_aspect.is_finite()
            || image_aspect <= 0.0
            || !viewport_aspect.is_finite()
            || viewport_aspect <= 0.0
            || view.validate().is_err()
        {
            return None;
        }
        let fit = if image_aspect > viewport_aspect {
            [1.0, viewport_aspect / image_aspect]
        } else {
            [image_aspect / viewport_aspect, 1.0]
        };
        Some(Self {
            size,
            scale: fit.map(|v| v * view.zoom as f64),
            pan: view.pan.map(f64::from),
        })
    }
    pub fn pixel_to_view(&self, pixel: [f64; 2]) -> Option<[f64; 2]> {
        if pixel.iter().any(|v| !v.is_finite()) {
            return None;
        }
        Some(std::array::from_fn(|i| {
            ((pixel[i] + 0.5) / self.size[i] as f64 - 0.5) * self.scale[i] + 0.5 + self.pan[i]
        }))
    }
    pub fn view_to_pixel(&self, uv: [f64; 2]) -> Option<[f64; 2]> {
        if uv.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let pixel: [f64; 2] = std::array::from_fn(|i| {
            ((uv[i] - 0.5 - self.pan[i]) / self.scale[i] + 0.5) * self.size[i] as f64 - 0.5
        });
        pixel
            .iter()
            .enumerate()
            .all(|(i, v)| *v >= 0.0 && *v <= (self.size[i] - 1) as f64)
            .then_some(pixel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapping_respects_letterboxing_pan_zoom_and_pixel_centres() {
        let view = ViewState {
            zoom: 2.0,
            pan: [0.1, -0.2],
            ..Default::default()
        };
        let map = ImageMapping::new([200, 100], 2.0, 1.0, &view).unwrap();
        let uv = map.pixel_to_view([99.5, 49.5]).unwrap();
        assert!((uv[0] - 0.6).abs() < 1e-7 && (uv[1] - 0.3).abs() < 1e-7);
        for pixel in [[0., 0.], [199., 99.], [43.5, 68.25]] {
            let actual = map
                .view_to_pixel(map.pixel_to_view(pixel).unwrap())
                .unwrap();
            assert!((actual[0] - pixel[0]).abs() < 1e-8 && (actual[1] - pixel[1]).abs() < 1e-8);
        }
        let map = ImageMapping::new([200, 100], 2.0, 1.0, &ViewState::default()).unwrap();
        assert!(map.view_to_pixel([0.5, 0.1]).is_none());
        assert_eq!(map.pixel_to_view([99.5, 49.5]), Some([0.5, 0.5]));
    }
}
