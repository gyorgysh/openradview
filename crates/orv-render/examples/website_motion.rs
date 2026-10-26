// SPDX-License-Identifier: GPL-3.0-or-later
//! Native frames from the licensed embedded NLST CT only. No user scan input.
//! MPR pixels, crosshairs and pointer are composited into one decoder timeline.
use image::{Rgba, RgbaImage};
use orv_core::{
    Plane, ViewState, WindowLevel,
    load::load_zip,
    scene::{Reconstruction, SceneState, hex, series_fingerprint},
    view_mapping::ImageMapping,
    volume::Volume,
    volume_view::VolumeView,
};
use orv_render::snapshot::SnapshotRenderer;
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
const SCALE: u32 = 2;
const FRAMES: usize = 330;
fn write(path: PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}
fn line(image: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32) {
    for yy in y..(y + height).min(image.height()) {
        for xx in x..(x + width).min(image.width()) {
            let p = image.get_pixel_mut(xx, yy);
            for c in 0..3 {
                p[c] = (p[c] as f32 * 0.35 + 255. * 0.65).round() as u8;
            }
        }
    }
}
fn pointer(image: &mut RgbaImage, tip: [f64; 2]) {
    // Same original arrow geometry as the website SVG, rasterized with 4x4
    // coverage at the video's 2x resolution. The arrow tip is the picked point.
    let polygon = [
        [4., 2.5],
        [4., 19.7],
        [8.6, 15.4],
        [11.5, 22.],
        [14.5, 20.7],
        [11.6, 14.2],
        [17.9, 14.2],
    ];
    let origin = [tip[0] - 4. * SCALE as f64, tip[1] - 2.5 * SCALE as f64];
    for y in 0..24 * SCALE {
        for x in 0..24 * SCALE {
            let mut white = 0;
            let mut black = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let p = [
                        (x as f64 + (sx as f64 + 0.5) / 4.) / SCALE as f64,
                        (y as f64 + (sy as f64 + 0.5) / 4.) / SCALE as f64,
                    ];
                    let mut inside = false;
                    let mut distance = f64::INFINITY;
                    for i in 0..polygon.len() {
                        let a = polygon[i];
                        let b = polygon[(i + 1) % polygon.len()];
                        if (a[1] > p[1]) != (b[1] > p[1])
                            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
                        {
                            inside = !inside;
                        }
                        let d = [b[0] - a[0], b[1] - a[1]];
                        let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1])
                            / (d[0] * d[0] + d[1] * d[1]))
                            .clamp(0., 1.);
                        distance =
                            distance.min((p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1]));
                    }
                    if distance <= 0.7 {
                        black += 1;
                    } else if inside {
                        white += 1;
                    }
                }
            }
            let xx = origin[0].round() as i64 + x as i64;
            let yy = origin[1].round() as i64 + y as i64;
            if xx >= 0 && yy >= 0 && xx < image.width() as i64 && yy < image.height() as i64 {
                let a = (white + black) as f32 / 16.;
                let p = image.get_pixel_mut(xx as u32, yy as u32);
                for c in 0..3 {
                    p[c] = (p[c] as f32 * (1. - a) + 255. * white as f32 / 16.).round() as u8;
                }
            }
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let folder = PathBuf::from(args.first().ok_or("Pass a fresh output folder")?);
    let preview = args.get(1).is_some_and(|v| v == "preview");
    std::fs::create_dir_all(folder.join("mpr"))?;
    std::fs::create_dir_all(folder.join("mip"))?;
    let study = load_zip(
        include_bytes!("../../../assets/demos/nlst-ct.zip"),
        &AtomicBool::new(false),
        |_| {},
    )?;
    let series = study.series.first().ok_or("Missing public CT")?;
    let fingerprint = series_fingerprint(series, || false)?;
    let volume = Arc::new(Volume::new(series.frames.clone())?);
    let mut renderer = pollster::block_on(SnapshotRenderer::new())?;
    let dimensions = volume.dimensions();
    let mut records = Vec::new();
    let mpr_indices: Vec<_> = if preview {
        vec![0, FRAMES / 2, FRAMES - 1]
    } else {
        (0..FRAMES).collect()
    };
    for i in mpr_indices {
        let u = i as f64 / (FRAMES - 1) as f64;
        let t = (1. - (std::f64::consts::PI * u).cos()) / 2.;
        let source = [256. - 56. * t, 256. + 44. * t, 24. + 33. * t];
        let point =
            volume.voxel_to_world([source[0], source[1], dimensions[2] as f64 - 1. - source[2]]);
        let mut canvas = RgbaImage::from_pixel(1036 * SCALE, 530 * SCALE, Rgba([0, 0, 0, 255]));
        let mut planes = Vec::new();
        let mut cursor = [0.; 2];
        for (plane, position, size) in [
            (Plane::Axial, [134, 24], [236, 236]),
            (Plane::Coronal, [632, 24], [300, 236]),
            (Plane::Sagittal, [102, 292], [300, 236]),
        ] {
            let frame = volume.reformat(plane, point, 512, || false)?;
            let scene = SceneState {
                schema_version: 1,
                source_sha256: fingerprint.clone(),
                series_index: 0,
                view: ViewState {
                    plane,
                    window: WindowLevel {
                        width: 1600.,
                        center: -400.,
                    },
                    ..Default::default()
                },
                crosshair_mm: Some(point),
                reformat_side: 512,
                output_size: size.map(|v| v * SCALE),
                reconstruction: None,
            };
            let snapshot = renderer.capture(&frame, &scene, || false)?;
            let image = image::load_from_memory(&snapshot.png)?.into_rgba8();
            let pixel = frame
                .geometry
                .as_ref()
                .ok_or("Missing reformat geometry")?
                .world_to_pixel(point)?;
            let map = ImageMapping::new(
                [frame.width, frame.height],
                frame.aspect_ratio() as f64,
                size[0] as f64 / size[1] as f64,
                &scene.view,
            )
            .ok_or("Invalid view mapping")?;
            let uv = map.pixel_to_view(pixel).ok_or("Invalid crosshair")?;
            let origin = position.map(|v| v * SCALE);
            image::imageops::overlay(&mut canvas, &image, origin[0] as i64, origin[1] as i64);
            let cross = [
                origin[0] as f64 + uv[0] * (size[0] * SCALE) as f64,
                origin[1] as f64 + uv[1] * (size[1] * SCALE) as f64,
            ];
            line(
                &mut canvas,
                cross[0].round() as u32,
                origin[1],
                SCALE,
                size[1] * SCALE,
            );
            line(
                &mut canvas,
                origin[0],
                cross[1].round() as u32,
                size[0] * SCALE,
                SCALE,
            );
            if plane == Plane::Axial {
                cursor = cross;
            }
            planes.push(serde_json::json!({"plane":plane,"png_sha256":snapshot.manifest.png_sha256,"crosshair_uv":uv}));
        }
        pointer(&mut canvas, cursor);
        let mut output = Cursor::new(Vec::new());
        canvas.write_to(&mut output, image::ImageFormat::Png)?;
        let png = output.into_inner();
        write(folder.join(format!("mpr/{i:04}.png")), &png)?;
        records.push(serde_json::json!({"frame":i,"source_pixel_and_slice":source,"point_lps_mm":point,"pointer_tip_canvas_px":cursor.map(|v|v/SCALE as f64),"planes":planes,"png_sha256":hex(&Sha256::digest(&png))}));
        if i % 60 == 0 {
            eprintln!("Rendered linked MPR frame {i}/{FRAMES}");
        }
    }
    let mut state = VolumeView::fit(&volume);
    state.camera.vertical_span_mm = 270.;
    let mip_indices: Vec<_> = if preview { vec![0] } else { (0..408).collect() };
    for i in mip_indices {
        state.camera.yaw_degrees = i as f64 * 360. / 408.;
        let scene = SceneState {
            schema_version: 2,
            source_sha256: fingerprint.clone(),
            series_index: 0,
            view: ViewState {
                window: WindowLevel {
                    width: 1400.,
                    center: 900.,
                },
                ..Default::default()
            },
            crosshair_mm: None,
            reformat_side: 2,
            output_size: [560, 438],
            reconstruction: Some(Reconstruction::Volume(state.clone())),
        };
        let snapshot = renderer.capture_volume(volume.clone(), &scene, || false)?;
        write(folder.join(format!("mip/{i:04}.png")), &snapshot.png)?;
        if i % 60 == 0 {
            eprintln!("Rendered dense-bone MIP frame {i}/408");
        }
    }
    let provenance = serde_json::json!({"source":"Bundled licensed public NLST CT; assets/demos/manifest.json and CREDITS.txt","source_sha256":fingerprint,"preview_only":preview,"mpr":{"frames":FRAMES,"seconds":5.5,"start":1.3,"end":6.8,"canvas":[1036*SCALE,530*SCALE],"method":"All three native physical MPR planes, pointer and crosshairs in one frame; same world point; no optical flow","window":{"width":1600,"level":-400},"readouts":records},"mip":{"frames":408,"seconds":6.8,"window":{"width":1400,"level":900},"vertical_span_mm":270,"method":"Native MIP, closer fixed framing and original custom high-density window; no synthesized anatomy"}});
    write(
        folder.join("provenance.json"),
        serde_json::to_string_pretty(&provenance)?.as_bytes(),
    )?;
    Ok(())
}
