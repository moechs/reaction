use crate::Value;

impl Value {
    /// Merge two configurations.
    /// Useful to merge two top-level options in a plugin.
    pub fn merge(self, other: Self) -> Result<Self, String> {
        self._merge(other).map_err(|(key, val1, val2)|
            format!(
                "Cannot merge keys {key} `{}` and `{}`: types {} and {} are not mergeable. Only 2 objects, 2 arrays, or an array with *anything* can be merged together.",
                val1.value_type(),
                val2.value_type(),
                Into::<serde_json::Value>::into(val1),
                Into::<serde_json::Value>::into(val2)
            )
        )
    }
    fn _merge(self, other: Self) -> Result<Self, (String, Value, Value)> {
        if self == other {
            return Ok(self);
        }
        match (self, other) {
            // Null
            (Value::Null, other) => Ok(other),
            (this, Value::Null) => Ok(this),

            // Arrays
            (Value::Array(mut array), Value::Array(array2)) => {
                array.extend(array2);
                Ok(Value::Array(array))
            }
            (Value::Array(mut array), other) => {
                array.push(other);
                Ok(Value::Array(array))
            }
            (other, Value::Array(mut array)) => {
                array.push(other);
                Ok(Value::Array(array))
            }

            // Objects
            (Value::Object(mut object), Value::Object(object2)) => {
                for (key, value) in object2.into_iter() {
                    let merged = match object.remove(&key) {
                        None => value,
                        Some(value2) => value._merge(value2).map_err(|(key2, val1, val2)| {
                            (format!("{}.{}", key, key2), val1, val2)
                        })?,
                    };
                    object.insert(key, merged);
                }
                Ok(Value::Object(object))
            }

            // Rest
            (this, other) => Err(("".into(), this, other)),
        }
    }

    fn value_type(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Integer(_) => "integer",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(l0), Self::Bool(r0)) => l0 == r0,
            (Self::Integer(l0), Self::Integer(r0)) => l0 == r0,
            (Self::Float(l0), Self::Float(r0)) => l0 == r0,
            (Self::String(l0), Self::String(r0)) => l0 == r0,
            (Self::Array(l0), Self::Array(r0)) => l0 == r0,
            (Self::Object(l0), Self::Object(r0)) => l0 == r0,
            (Self::Null, Self::Null) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::Value;

    #[test]
    fn merge_bools() {
        assert!(Value::Bool(true).merge(Value::Bool(true)) == Ok(Value::Bool(true)));
        assert!(Value::Bool(false).merge(Value::Bool(false)) == Ok(Value::Bool(false)));

        assert!(Value::Bool(true).merge(Value::Bool(false)).is_err());
        assert!(Value::Bool(true).merge(Value::Integer(1)).is_err());
        assert!(Value::Bool(true).merge(Value::Float(1.0)).is_err());
        assert!(Value::Bool(true).merge(Value::String("".into())).is_err());
        assert!(
            Value::Bool(true)
                .merge(Value::Object(Default::default()))
                .is_err()
        );
    }

    #[test]
    fn merge_integers() {
        assert!(Value::Integer(1).merge(Value::Integer(1)) == Ok(Value::Integer(1)));
        assert!(Value::Integer(2986).merge(Value::Integer(2986)) == Ok(Value::Integer(2986)));

        assert!(Value::Integer(1).merge(Value::Integer(2)).is_err());
        assert!(Value::Integer(1).merge(Value::Bool(true)).is_err());
        assert!(Value::Integer(1).merge(Value::Float(1.0)).is_err());
        assert!(Value::Integer(1).merge(Value::String("".into())).is_err());
        assert!(
            Value::Integer(1)
                .merge(Value::Object(Default::default()))
                .is_err()
        );
    }
    // same thing for floats and strings, let's skip those

    #[test]
    fn merge_nulls() {
        assert!(Value::Null.merge(Value::Null) == Ok(Value::Null));

        assert!(Value::Null.merge(Value::Integer(2)) == Ok(Value::Integer(2)));
        assert!(Value::Null.merge(Value::Bool(true)) == Ok(Value::Bool(true)));
        assert!(Value::Null.merge(Value::Float(1.0)) == Ok(Value::Float(1.0)));
        assert!(Value::Null.merge(Value::String("".into())) == Ok(Value::String("".into())));
        assert!(Value::Null.merge(Value::Array(vec![])) == Ok(Value::Array(vec![])));
        assert!(
            Value::Null.merge(Value::Object(Default::default()))
                == Ok(Value::Object(Default::default()))
        );

        assert!(Value::Integer(2).merge(Value::Null) == Ok(Value::Integer(2)));
        assert!(Value::Bool(true).merge(Value::Null) == Ok(Value::Bool(true)));
        assert!(Value::Float(1.0).merge(Value::Null) == Ok(Value::Float(1.0)));
        assert!(Value::String("".into()).merge(Value::Null) == Ok(Value::String("".into())));
        assert!(Value::Array(vec![]).merge(Value::Null) == Ok(Value::Array(vec![])));
        assert!(
            Value::Object(Default::default()).merge(Value::Null)
                == Ok(Value::Object(Default::default()))
        );
    }

    #[test]
    fn merge_arrays() {
        let truee = Value::Bool(true);
        let one = Value::Integer(1);
        let obj = Value::Object(BTreeMap::from([(
            "toto".into(),
            Value::String("titi".into()),
        )]));

        assert!(
            Value::Array(vec![truee.clone()]).merge(Value::Array(vec![one.clone(), obj.clone()]))
                == Ok(Value::Array(vec![truee.clone(), one.clone(), obj.clone()]))
        );

        assert!(
            Value::Array(vec![one.clone()]).merge(obj.clone())
                == Ok(Value::Array(vec![one.clone(), obj.clone()]))
        );
        assert!(
            obj.clone().merge(Value::Array(vec![one.clone()]))
                == Ok(Value::Array(vec![one.clone(), obj.clone()]))
        );
    }

    #[test]
    fn merge_objects() {
        let tuple_toto = ("toto".into(), Value::String("titi".into()));
        let tuple_totobis = ("toto".into(), Value::String("tata".into()));
        let tuple_a = ("a".into(), Value::String("aa".into()));
        let tuple_b = ("b".into(), Value::String("bb".into()));
        let obj_a = Value::Object(BTreeMap::from([tuple_a.clone(), tuple_toto.clone()]));
        let obj_b = Value::Object(BTreeMap::from([tuple_b.clone(), tuple_toto.clone()]));
        let obj_bbis = Value::Object(BTreeMap::from([tuple_b.clone(), tuple_totobis.clone()]));

        let _ = obj_a
            .clone()
            .merge(obj_b.clone())
            .inspect(|v| println!("OK {}", Into::<serde_json::Value>::into(v.to_owned())))
            .inspect_err(|e| println!("ERROR: {e}"));

        assert!(
            obj_a.clone().merge(obj_b)
                == Ok(Value::Object(BTreeMap::from([
                    tuple_a.clone(),
                    tuple_b.clone(),
                    tuple_toto.clone()
                ])))
        );

        assert!(obj_a.clone().merge(obj_bbis).is_err());
    }
}
