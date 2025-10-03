use std::collections::BTreeMap;

use reaction_plugin::Value as RValue;
use serde_json::Value as JValue;

pub fn to_stable_value(val: JValue) -> RValue {
    match val {
        JValue::Null => RValue::Null,
        JValue::Bool(b) => RValue::Bool(b),
        JValue::Number(number) => {
            if let Some(number) = number.as_i64() {
                RValue::Integer(number)
            } else if let Some(number) = number.as_f64() {
                RValue::Float(number)
            } else {
                RValue::Null
            }
        }
        JValue::String(s) => RValue::String(s.into()),
        JValue::Array(v) => RValue::Array({
            let mut vec = Vec::with_capacity(v.len());
            for val in v {
                vec.push(to_stable_value(val));
            }
            vec
        }),
        JValue::Object(m) => RValue::Object({
            let mut map = BTreeMap::new();
            for (key, val) in m {
                map.insert(key.into(), to_stable_value(val));
            }
            map
        }),
    }
}
