use crate::Value;

impl Value {
    /// Merge two configurations.
    /// Useful to merge two top-level options in a plugin.
    pub fn merge(self, other: Self) -> Result<Self, String> {
        self._merge(other).map_err(|(key, val1, val2)| format!("Cannot merge keys {key}: {} and {} are not mergeable. Only two objects, two arrays, or an array with *anything* can be merged together.", Into::<serde_json::Value>::into(val1), Into::<serde_json::Value>::into(val2)))
    }
    fn _merge(self, other: Self) -> Result<Self, (String, Value, Value)> {
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
                    match object.remove(&key) {
                        None => {
                            object.insert(key, value);
                        }
                        Some(value2) => {
                            if value != value2 {
                                let merged =
                                    value._merge(value2).map_err(|(key2, val1, val2)| {
                                        (format!("{}.{}", key, key2), val1, val2)
                                    })?;
                                object.insert(key, merged);
                            }
                        }
                    }
                }
                Ok(Value::Object(object))
            }

            // Rest
            (this, other) => Err(("".into(), this, other)),
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

// TODO Test
