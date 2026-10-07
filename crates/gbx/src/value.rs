//! XML-RPC values as exchanged with the dedicated server.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::{Serialize, de::DeserializeOwned};

use crate::error::ValueError;

/// An XML-RPC value.
///
/// The dedicated server uses the classic XML-RPC types. `<i4>`/`<int>` are 32-bit signed integers.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i32),
    Bool(bool),
    String(String),
    Double(f64),
    /// ISO 8601 date, kept verbatim (the server rarely sends any).
    DateTime(String),
    Base64(Vec<u8>),
    Array(Vec<Value>),
    Struct(BTreeMap<String, Value>),
    /// `<nil/>` extension; not used by the server but tolerated when parsing.
    Nil,
}

impl Value {
    /// Short type name used in error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Bool(_) => "boolean",
            Value::String(_) => "string",
            Value::Double(_) => "double",
            Value::DateTime(_) => "dateTime.iso8601",
            Value::Base64(_) => "base64",
            Value::Array(_) => "array",
            Value::Struct(_) => "struct",
            Value::Nil => "nil",
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_struct(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Struct(s) => Some(s),
            _ => None,
        }
    }

    /// Converts into a JSON value (base64 becomes a base64 string, datetime a string).
    pub fn into_json(self) -> serde_json::Value {
        match self {
            Value::Int(i) => i.into(),
            Value::Bool(b) => b.into(),
            Value::String(s) | Value::DateTime(s) => s.into(),
            Value::Double(d) => serde_json::Number::from_f64(d)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Base64(bytes) => base64::engine::general_purpose::STANDARD
                .encode(bytes)
                .into(),
            Value::Array(items) => items.into_iter().map(Value::into_json).collect(),
            Value::Struct(members) => serde_json::Value::Object(
                members
                    .into_iter()
                    .map(|(k, v)| (k, v.into_json()))
                    .collect(),
            ),
            Value::Nil => serde_json::Value::Null,
        }
    }

    /// Converts from a JSON value. Integers that fit in `i32` become `Int`, other numbers `Double`.
    pub fn from_json(json: serde_json::Value) -> Result<Value, ValueError> {
        Ok(match json {
            serde_json::Value::Null => Value::Nil,
            serde_json::Value::Bool(b) => Value::Bool(b),
            serde_json::Value::Number(n) => match n.as_i64().and_then(|i| i32::try_from(i).ok()) {
                Some(i) => Value::Int(i),
                None => Value::Double(n.as_f64().ok_or(ValueError::NumberOutOfRange)?),
            },
            serde_json::Value::String(s) => Value::String(s),
            serde_json::Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(Value::from_json)
                    .collect::<Result<_, _>>()?,
            ),
            serde_json::Value::Object(members) => Value::Struct(
                members
                    .into_iter()
                    .map(|(k, v)| Ok((k, Value::from_json(v)?)))
                    .collect::<Result<_, ValueError>>()?,
            ),
        })
    }

    /// Deserializes this value into a typed structure (through JSON).
    pub fn deserialize<T: DeserializeOwned>(self) -> Result<T, ValueError> {
        serde_json::from_value(self.into_json()).map_err(ValueError::Deserialize)
    }

    /// Serializes a typed structure into a value (through JSON).
    pub fn serialize<T: Serialize + ?Sized>(value: &T) -> Result<Value, ValueError> {
        Value::from_json(serde_json::to_value(value).map_err(ValueError::Serialize)?)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Double(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_owned())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Base64(v)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

impl From<BTreeMap<String, Value>> for Value {
    fn from(v: BTreeMap<String, Value>) -> Self {
        Value::Struct(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_keeps_ints_and_doubles_apart() {
        let v = Value::from_json(
            serde_json::json!({"a": 1, "b": 1.5, "c": [true, "x"], "d": 5_000_000_000_i64}),
        )
        .unwrap();
        let s = v.as_struct().unwrap();
        assert_eq!(s["a"], Value::Int(1));
        assert_eq!(s["b"], Value::Double(1.5));
        assert_eq!(
            s["c"],
            Value::Array(vec![Value::Bool(true), Value::from("x")])
        );
        assert_eq!(s["d"], Value::Double(5_000_000_000.0));
    }

    #[test]
    fn deserialize_typed_struct() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Version {
            name: String,
            build: String,
        }
        let mut m = BTreeMap::new();
        m.insert("Name".to_owned(), Value::from("Trackmania"));
        m.insert("Build".to_owned(), Value::from("2026-01-01_00_00"));
        let v: Version = Value::Struct(m).deserialize().unwrap();
        assert_eq!(v.name, "Trackmania");
        assert_eq!(v.build, "2026-01-01_00_00");
    }
}
