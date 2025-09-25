use stabby::{string::String, tuple::Tuple2, vec::Vec};

/// Represents a configuration value.
/// This is not meant as an efficient type, but as a very flexible one.
#[stabby::stabby]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<Tuple2<String, Value>>),
}
