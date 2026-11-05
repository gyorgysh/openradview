// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{reslice::GpuReformatImage, volume::allocation};
impl SliceRenderer {
    /// Prepare presentation on the completed plane's own GPU context. Raw
    /// Device equality is insufficient: native IDs can repeat across Instances.
    pub fn from_reslice(
        image: Arc<GpuReformatImage>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, &'static str> {
        let mut renderer = Self::new(image.source().device(), format);
        renderer.reslice_source = Some(image.source().clone());
        renderer.upload_reslice(image)?;
        Ok(renderer)
    }
    /// Bind an immutable completed plane without uploading its CPU samples.
    /// The exact matching Frame remains available for local tools and export.
    pub fn upload_reslice(&mut self, image: Arc<GpuReformatImage>) -> Result<(), &'static str> {
        if !self
            .reslice_source
            .as_ref()
            .is_some_and(|source| Arc::ptr_eq(source, image.source()))
        {
            return Err("Reslice image requires presentation bound to its exact source");
        }
        let device = image.source().device();
        let queue = image.source().queue();
        let frame = image.frame();
        if !frame.valid_dimensions()
            || frame.rgba.is_some()
            || image.grid().dimensions() != [frame.width, frame.height]
        {
            return Err("Reslice image and its completed frame differ");
        }
        let new = self.external.is_none();
        let reservation = if new {
            Some(allocation::reserve_bytes(65536, device, queue)?)
        } else {
            None
        };
        let dummy = if new {
            Some(allocation::texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some("unused mask for scalar texture validity"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Uint,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )?)
        } else {
            None
        };
        let mask = dummy
            .as_ref()
            .or(self.validity.as_ref())
            .ok_or("Reslice bindings unavailable")?
            .create_view(&Default::default());
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("completed scalar plane display"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(image.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&mask),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&image.source().voi),
                },
            ],
        });
        self.bindings = Some(bindings);
        self.texture = None;
        if let Some(dummy) = dummy {
            self.validity = Some(dummy);
        }
        if let Some(reservation) = reservation {
            self._reservation = Some(reservation);
        }
        self.size = [frame.width, frame.height];
        self.color = false;
        self.voi = Some(image.source().voi.clone());
        self.voi_signature = crate::voi::signature(frame.scalar.as_deref());
        self.external = Some(image);
        Ok(())
    }
}
