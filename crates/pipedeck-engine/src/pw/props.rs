//! Building the `Props` param that carries volume and mute for a node.

use std::io::Cursor;

use libspa::param::ParamType;
use libspa::pod::deserialize::PodDeserializer;
use libspa::pod::serialize::PodSerializer;
use libspa::pod::{Object, Pod, Property, Value, ValueArray};
use libspa::sys as spa_sys;
use libspa::utils::SpaTypes;

use crate::types::ChainState;

/// Serialize a `Props` object setting `channelVolumes` (one linear value per
/// channel) and `mute`. Returns the raw pod bytes, ready for
/// [`libspa::pod::Pod::from_bytes`].
pub fn volume_props(state: &ChainState, channels: usize) -> Vec<u8> {
    let volume = state.linear_volume();
    let object = Object {
        type_: SpaTypes::ObjectParamProps.as_raw(),
        id: ParamType::Props.as_raw(),
        properties: vec![
            Property::new(spa_sys::SPA_PROP_mute, Value::Bool(state.muted)),
            Property::new(
                spa_sys::SPA_PROP_channelVolumes,
                Value::ValueArray(ValueArray::Float(vec![volume; channels])),
            ),
        ],
    };
    let (cursor, _len) = PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .expect("serializing an in-memory pod cannot fail");
    cursor.into_inner()
}

/// Read a level back out of a `Props` object.
///
/// Returns nothing when the object carries neither of the two keys we care
/// about, which is the common case: a node emits `Props` for many reasons.
pub fn parse_volume(pod: &Pod) -> Option<ChainState> {
    let (_, value) = PodDeserializer::deserialize_any_from(pod.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };
    if object.id != ParamType::Props.as_raw() {
        return None;
    }

    let mut volume: Option<f32> = None;
    let mut muted: Option<bool> = None;
    for property in &object.properties {
        match (property.key, &property.value) {
            (spa_sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(values))) => {
                // The loudest channel stands for the node: a level the user
                // set is the same on every channel anyway.
                volume = values.iter().copied().fold(None, |acc: Option<f32>, v| {
                    Some(acc.map_or(v, |max| max.max(v)))
                });
            }
            (spa_sys::SPA_PROP_mute, Value::Bool(value)) => muted = Some(*value),
            _ => {}
        }
    }

    let volume = volume?;
    Some(ChainState {
        // The inverse of the cubic curve the faders use.
        gain: volume.clamp(0.0, 1.0).cbrt(),
        muted: muted.unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_pod_roundtrips() {
        let state = ChainState {
            gain: 0.5,
            muted: true,
        };
        let bytes = volume_props(&state, 2);
        let pod = Pod::from_bytes(&bytes).expect("valid pod");
        let (_, value) =
            PodDeserializer::deserialize_any_from(pod.as_bytes()).expect("deserialize");
        let Value::Object(obj) = value else {
            panic!("not an object");
        };
        assert_eq!(obj.type_, spa_sys::SPA_TYPE_OBJECT_Props);
        assert_eq!(obj.id, spa_sys::SPA_PARAM_Props);
        assert_eq!(obj.properties.len(), 2);
        assert_eq!(obj.properties[0].value, Value::Bool(true));
        assert_eq!(
            obj.properties[1].value,
            Value::ValueArray(ValueArray::Float(vec![0.125, 0.125]))
        );
    }

    #[test]
    fn a_level_survives_the_round_trip() {
        let state = ChainState {
            gain: 0.5,
            muted: true,
        };
        let bytes = volume_props(&state, 2);
        let read = parse_volume(Pod::from_bytes(&bytes).unwrap()).expect("a level");
        assert!((read.gain - state.gain).abs() < 1e-3, "{read:?}");
        assert!(read.muted);
    }

    #[test]
    fn an_object_without_a_level_is_ignored() {
        let object = Object {
            type_: SpaTypes::ObjectParamProps.as_raw(),
            id: ParamType::Props.as_raw(),
            properties: vec![Property::new(
                spa_sys::SPA_PROP_channelMap,
                Value::Bool(false),
            )],
        };
        let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
            .unwrap()
            .0
            .into_inner();
        assert!(parse_volume(Pod::from_bytes(&bytes).unwrap()).is_none());
    }
}
