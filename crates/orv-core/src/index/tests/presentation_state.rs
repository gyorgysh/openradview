// SPDX-License-Identifier: GPL-3.0-or-later
use super::real_world::phantom;
use super::*;
use crate::{
    VoiFunction,
    index::{PresentationError, read_presentation_state},
};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object, meta::FileMetaTableBuilder};

const GSPS: &str = "1.2.840.10008.5.1.4.1.1.11.1";
const SECONDARY: &str = "1.2.840.10008.5.1.4.1.1.7";

fn write(path: &std::path::Path, object: &DefaultDicomObject) {
    let mut data = Vec::new();
    object.write_all(&mut data).unwrap();
    fs::write(path, data).unwrap();
}
/// Two 4x4 single-frame images in one series, instances 2.25.7001/2.
fn images(folder: &std::path::Path) {
    for instance in 1..=2u32 {
        let mut object = phantom::integer(
            "1.2.840.10008.1.2.1",
            phantom::item(1., 0., Some((0, 255)), None),
        );
        object.remove_element(Tag(0x0040, 0x9096));
        for tag in [Tag(0x0028, 0x0010), Tag(0x0028, 0x0011)] {
            object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(4u16)));
        }
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            PrimitiveValue::U8(vec![instance as u8; 16].into()),
        ));
        let uid = format!("2.25.700{instance}");
        object.put_str(Tag(0x0008, 0x0018), VR::UI, &uid);
        object.update_meta(|meta| meta.media_storage_sop_instance_uid = uid);
        object.put_str(Tag(0x0020, 0x0013), VR::IS, instance.to_string());
        object.put_str(
            Tag(0x0020, 0x0032),
            VR::DS,
            format!("0\\0\\{}", f64::from(instance) * 2.),
        );
        write(&folder.join(format!("image-{instance}.dcm")), &object);
    }
}
fn state(references: &[(&str, Option<u32>)]) -> DefaultDicomObject {
    let mut root = Object::new_empty();
    root.put_str(Tag(0x0008, 0x0016), VR::UI, GSPS);
    root.put_str(Tag(0x0008, 0x0018), VR::UI, "2.25.7100");
    root.put_str(Tag(0x0070, 0x0080), VR::CS, "GENERATED_STATE");
    let images = references
        .iter()
        .map(|(instance, frame)| {
            let mut item = Object::new_empty();
            item.put_str(Tag(0x0008, 0x1150), VR::UI, SECONDARY);
            item.put_str(Tag(0x0008, 0x1155), VR::UI, *instance);
            if let Some(frame) = frame {
                item.put_str(Tag(0x0008, 0x1160), VR::IS, frame.to_string());
            }
            item
        })
        .collect();
    let mut series = Object::new_empty();
    series.put_str(Tag(0x0020, 0x000e), VR::UI, "2.25.636101");
    series.put(phantom::sequence(Tag(0x0008, 0x1140), images));
    root.put(phantom::sequence(Tag(0x0008, 0x1115), vec![series]));
    root.put_str(Tag(0x2050, 0x0020), VR::CS, "INVERSE");
    let mut voi = Object::new_empty();
    voi.put_str(Tag(0x0028, 0x1050), VR::DS, "1.5");
    voi.put_str(Tag(0x0028, 0x1051), VR::DS, "0.5");
    voi.put_str(Tag(0x0028, 0x1056), VR::CS, "LINEAR_EXACT");
    root.put(phantom::sequence(Tag(0x0028, 0x3110), vec![voi]));
    let mut area = Object::new_empty();
    area.put(DataElement::new(
        Tag(0x0070, 0x0052),
        VR::SL,
        PrimitiveValue::I32(vec![1, 1].into()),
    ));
    area.put(DataElement::new(
        Tag(0x0070, 0x0053),
        VR::SL,
        PrimitiveValue::I32(vec![4, 4].into()),
    ));
    area.put_str(Tag(0x0070, 0x0100), VR::CS, "SCALE TO FIT");
    root.put(phantom::sequence(Tag(0x0070, 0x005a), vec![area]));
    root.put(DataElement::new(
        Tag(0x0070, 0x0042),
        VR::US,
        PrimitiveValue::from(90u16),
    ));
    root.put_str(Tag(0x0070, 0x0041), VR::CS, "Y");
    root.with_meta(
        FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid(GSPS)
            .media_storage_sop_instance_uid("2.25.7100"),
    )
    .unwrap()
}
type Edit = Box<dyn Fn(&mut DefaultDicomObject)>;
/// Discard each mutation's previous value so cases share one closure type.
fn edit<R>(change: impl Fn(&mut DefaultDicomObject) -> R + 'static) -> Edit {
    Box::new(move |object| {
        change(object);
    })
}
fn load(object: &DefaultDicomObject) -> Result<crate::index::PresentationState, PresentationError> {
    let temp = Temp::new();
    images(&temp.0);
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    let path = temp.0.join("state.pre");
    write(&path, object);
    read_presentation_state(&path, &study.series[0], || false)
}

#[test]
fn a_supported_state_maps_references_window_inversion_and_spatial_order() {
    let applied = load(&state(&[("2.25.7002", None)])).unwrap();
    assert_eq!(applied.images, vec![1]);
    assert_eq!(
        (
            applied.window.function,
            applied.window.center,
            applied.window.width
        ),
        (VoiFunction::LinearExact, 1.5, 0.5)
    );
    assert!(applied.inverse);
    // Independent definition: flip horizontally, then rotate 90° clockwise
    // (screen y down), on centred coordinates.
    let flip_then_rotate = |[x, y]: [f64; 2]| {
        let [x, y] = [-x, y];
        [-y, x]
    };
    for point in [[1., 0.], [0., 1.], [2., -3.], [-0.5, 0.25]] {
        assert_eq!(applied.transform.apply(point), flip_then_rotate(point));
    }
    let both = load(&state(&[("2.25.7001", Some(1)), ("2.25.7002", None)])).unwrap();
    assert_eq!(both.images, vec![0, 1]);
    let json = format!("{both:?}");
    assert!(!json.contains("GENERATED_STATE") && !json.contains("2.25.700"));
}

#[test]
fn unsupported_or_unrelated_states_are_refused_whole() {
    let base = state(&[("2.25.7001", None)]);
    let cases: Vec<(Edit, PresentationError)> = vec![
        (
            edit(|o| o.put_str(Tag(0x0070, 0x0001), VR::SQ, "")),
            PresentationError::Unsupported("graphic or text annotations"),
        ),
        (
            edit(|o| o.put_str(Tag(0x0018, 0x1600), VR::CS, "RECTANGULAR")),
            PresentationError::Unsupported("display shutters"),
        ),
        (
            edit(|o| {
                o.put(DataElement::new(
                    Tag(0x6000, 0x0010),
                    VR::US,
                    PrimitiveValue::from(4u16),
                ))
            }),
            PresentationError::Unsupported("overlay planes"),
        ),
        (
            edit(|o| o.put_str(Tag(0x0028, 0x1053), VR::DS, "1")),
            PresentationError::Unsupported("a modality rescale"),
        ),
        (
            edit(|o| {
                let mut area = Object::new_empty();
                area.put_str(Tag(0x0070, 0x0100), VR::CS, "TRUE SIZE");
                o.put(phantom::sequence(Tag(0x0070, 0x005a), vec![area]));
            }),
            PresentationError::Unsupported("a true-size or magnified display"),
        ),
        (
            edit(|o| {
                let mut area = Object::new_empty();
                area.put(DataElement::new(
                    Tag(0x0070, 0x0052),
                    VR::SL,
                    PrimitiveValue::I32(vec![2, 1].into()),
                ));
                area.put(DataElement::new(
                    Tag(0x0070, 0x0053),
                    VR::SL,
                    PrimitiveValue::I32(vec![4, 4].into()),
                ));
                area.put_str(Tag(0x0070, 0x0100), VR::CS, "SCALE TO FIT");
                o.put(phantom::sequence(Tag(0x0070, 0x005a), vec![area]));
            }),
            PresentationError::Unsupported("a displayed area other than the whole image"),
        ),
        (
            edit(|o| {
                o.remove_element(Tag(0x0028, 0x3110));
            }),
            PresentationError::Unsupported("no softcopy window"),
        ),
        (
            edit(|o| {
                let state = state(&[("2.25.9999", None)]);
                *o = state;
            }),
            PresentationError::Association,
        ),
        (
            edit(|o| {
                o.put(DataElement::new(
                    Tag(0x0070, 0x0042),
                    VR::US,
                    PrimitiveValue::from(45u16),
                ))
            }),
            PresentationError::Metadata,
        ),
        (
            edit(|o| {
                *o = state(&[("2.25.7001", Some(2))]);
            }),
            PresentationError::Association,
        ),
    ];
    for (index, (change, expected)) in cases.into_iter().enumerate() {
        let mut object = base.clone();
        change(&mut object);
        assert_eq!(load(&object), Err(expected), "case {index}");
    }
    // An ordinary image is not a presentation state.
    let temp = Temp::new();
    images(&temp.0);
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    assert_eq!(
        read_presentation_state(&temp.0.join("image-1.dcm"), &study.series[0], || false),
        Err(PresentationError::Kind)
    );
}
