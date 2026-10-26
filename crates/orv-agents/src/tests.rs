// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    ViewState,
    scene::{ImageMetadata, SceneState},
};
use orv_render::snapshot::SnapshotManifest;

fn snapshot() -> Snapshot {
    let scene = SceneState {
        schema_version: 1,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState::default(),
        crosshair_mm: None,
        reformat_side: 64,
        output_size: [2, 2],
        reconstruction: None,
    };
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 128, 128, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    Snapshot {
        manifest: SnapshotManifest {
            renderer_version: 1,
            scene_sha256: scene.hash().unwrap(),
            png_sha256: hex(&Sha256::digest(&png)),
            scene,
            image: Some(ImageMetadata {
                color: None,
                nifti: None,
                color_space: None,
                dimensions: [2, 2],
                monochrome1: false,
                geometry: None,
            }),
            volume: None,
            includes_ui_overlays: false,
        },
        png,
    }
}
fn consent() -> Consent {
    Consent {
        recipients: vec!["Client A".into(), "Client B".into()],
        preview_reviewed: true,
        onward_sharing_accepted: true,
    }
}
fn reply(snapshot: &ReviewedSnapshot) -> AgentReply {
    AgentReply {
        scene_sha256: snapshot.snapshot().manifest.scene_sha256.clone(),
        png_sha256: snapshot.snapshot().manifest.png_sha256.clone(),
        model: "Synthetic fixture".into(),
        commentary: "Experimental AI commentary: synthetic squares; no clinical interpretation."
            .into(),
    }
}

#[test]
fn consent_denies_implicit_or_ambiguous_recipients_and_shares_identical_bytes() {
    let snapshot = ReviewedSnapshot::new(snapshot()).unwrap();
    for labels in [
        vec![],
        vec!["one"],
        vec!["one", " ONE "],
        vec!["one", "\n"],
        vec!["1", "2", "3", "4", "5"],
    ] {
        let mut c = consent();
        c.recipients = labels.into_iter().map(str::to_string).collect();
        assert!(Comparison::grant(snapshot.clone(), c).is_err());
    }
    for flags in [[false, true], [true, false], [false, false]] {
        let mut c = consent();
        c.preview_reviewed = flags[0];
        c.onward_sharing_accepted = flags[1];
        assert!(Comparison::grant(snapshot.clone(), c).is_err());
    }
    let comparison = Comparison::grant(snapshot.clone(), consent()).unwrap();
    let a = comparison.recipients()[0].read_snapshot().unwrap();
    let b = comparison.recipients()[1].read_snapshot().unwrap();
    assert!(Arc::ptr_eq(&snapshot, &a) && Arc::ptr_eq(&a, &b));
    assert_eq!(a.snapshot().png, b.snapshot().png);
    assert_eq!(a.snapshot().manifest, b.snapshot().manifest);
}

#[test]
fn rejects_changed_pixels_scene_dimensions_and_overlays_before_consent() {
    let mut s = snapshot();
    s.png[32] ^= 1;
    assert!(ReviewedSnapshot::new(s).is_err());
    let mut s = snapshot();
    s.manifest.scene.view.slice += 1;
    assert!(ReviewedSnapshot::new(s).is_err());
    let mut s = snapshot();
    s.manifest.scene.output_size = [3, 2];
    s.manifest.scene_sha256 = s.manifest.scene.hash().unwrap();
    assert!(ReviewedSnapshot::new(s).is_err());
    let mut s = snapshot();
    s.manifest.includes_ui_overlays = true;
    assert!(ReviewedSnapshot::new(s).is_err());
    let mut s = snapshot();
    s.png.clear();
    s.manifest.png_sha256 = hex(&Sha256::digest(&s.png));
    assert!(ReviewedSnapshot::new(s).is_err());
}

#[test]
fn commentary_is_bounded_provenance_checked_final_and_isolated() {
    let snapshot = ReviewedSnapshot::new(snapshot()).unwrap();
    let comparison = Comparison::grant(snapshot.clone(), consent()).unwrap();
    let a = &comparison.recipients()[0];
    let b = &comparison.recipients()[1];
    let valid = reply(&snapshot);
    assert!(a.submit(valid.clone()).is_err());
    a.read_snapshot().unwrap();
    let mut invalid = valid.clone();
    invalid.scene_sha256 = "1".repeat(64);
    assert!(a.submit(invalid).is_err());
    let mut invalid = valid.clone();
    invalid.png_sha256 = "1".repeat(64);
    assert!(a.submit(invalid).is_err());
    for commentary in [
        "x".repeat(32_769),
        "\u{202e}spoof".into(),
        "\0control".into(),
        "  ".into(),
    ] {
        let mut invalid = valid.clone();
        invalid.commentary = commentary;
        assert!(a.submit(invalid).is_err());
    }
    assert_eq!(a.status(), RecipientStatus::SnapshotRead);
    assert_eq!(b.status(), RecipientStatus::Waiting);
    a.submit(valid.clone()).unwrap();
    a.submit(valid.clone()).unwrap();
    let mut replacement = valid.clone();
    replacement.commentary.push_str(" changed");
    assert!(a.submit(replacement).is_err());
    assert_eq!(a.status(), RecipientStatus::Submitted(valid));
    assert_eq!(b.status(), RecipientStatus::Waiting);
}

#[test]
fn revoke_and_drop_release_pixels_and_block_cloned_capabilities() {
    let snapshot = ReviewedSnapshot::new(snapshot()).unwrap();
    let weak = Arc::downgrade(&snapshot);
    let valid = reply(&snapshot);
    let comparison = Comparison::grant(snapshot, consent()).unwrap();
    let a = comparison.recipients()[0].clone();
    let b = comparison.recipients()[1].clone();
    a.read_snapshot().unwrap();
    a.submit(valid.clone()).unwrap();
    a.revoke();
    assert!(a.read_snapshot().is_err() && a.submit(valid.clone()).is_err());
    assert!(b.read_snapshot().is_ok());
    assert!(weak.upgrade().is_some());
    drop(comparison);
    assert!(b.is_revoked() && b.cancellation().is_cancelled());
    assert_eq!(a.status(), RecipientStatus::Revoked);
    assert_eq!(b.status(), RecipientStatus::Revoked);
    assert!(b.read_snapshot().is_err() && b.submit(valid).is_err());
    assert!(
        weak.upgrade().is_none(),
        "Revoked handles must not retain the PNG"
    );
}
