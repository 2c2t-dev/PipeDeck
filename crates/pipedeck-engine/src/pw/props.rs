//! Building the `Props` param that carries volume and mute for a node.

use std::io::Cursor;

use libspa::param::ParamType;
use libspa::pod::serialize::PodSerializer;
use libspa::pod::{Object, Property, Value, ValueArray};
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

#[cfg(test)]
mod tests {
    use super::*;
    use libspa::pod::deserialize::PodDeserializer;
    use libspa::pod::Pod;

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
}
