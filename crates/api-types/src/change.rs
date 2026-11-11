//! [`Change`]: a field of an update that can be kept, cleared or set.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One field of an update.
///
/// On the wire, a missing field or `null` keeps the value, an empty string
/// clears it, and any other value sets it. Older clients send `null` for
/// every field they leave alone, so `null` must keep. Use it with
/// `#[serde(default, skip_serializing_if = "Change::is_keep")]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Change<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

impl<T> Change<T> {
    pub fn is_keep(&self) -> bool {
        matches!(self, Change::Keep)
    }

    pub fn as_ref(&self) -> Change<&T> {
        match self {
            Change::Keep => Change::Keep,
            Change::Clear => Change::Clear,
            Change::Set(v) => Change::Set(v),
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Change<U> {
        match self {
            Change::Keep => Change::Keep,
            Change::Clear => Change::Clear,
            Change::Set(v) => Change::Set(f(v)),
        }
    }

    /// `None` to keep, `Some(None)` to clear, `Some(Some(v))` to set.
    pub fn into_option(self) -> Option<Option<T>> {
        match self {
            Change::Keep => None,
            Change::Clear => Some(None),
            Change::Set(v) => Some(Some(v)),
        }
    }

    /// `None` keeps, `Some(None)` clears, `Some(Some(v))` sets.
    pub fn from_option(o: Option<Option<T>>) -> Self {
        match o {
            None => Change::Keep,
            Some(None) => Change::Clear,
            Some(Some(v)) => Change::Set(v),
        }
    }
}

impl Change<String> {
    /// `None` keeps, an empty or blank string clears, and any other text
    /// sets.
    pub fn text(o: Option<String>) -> Self {
        match o {
            None => Change::Keep,
            Some(s) if s.trim().is_empty() => Change::Clear,
            Some(s) => Change::Set(s),
        }
    }

    /// `None` to keep, `Some(None)` to clear, `Some(Some(text))` to set.
    /// The text is trimmed, and blank text clears.
    pub fn as_deref(&self) -> Option<Option<&str>> {
        match self {
            Change::Keep => None,
            Change::Clear => Some(None),
            Change::Set(s) => Some(Some(s.trim()).filter(|s| !s.is_empty())),
        }
    }
}

impl<T: Serialize> Serialize for Change<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Change::Keep => s.serialize_none(),
            Change::Clear => s.serialize_str(""),
            Change::Set(v) => v.serialize(s),
        }
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Change<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Option::<serde_json::Value>::deserialize(d)?;
        let v = match v {
            None | Some(serde_json::Value::Null) => return Ok(Change::Keep),
            Some(serde_json::Value::String(s)) if s.trim().is_empty() => {
                return Ok(Change::Clear);
            }
            Some(v) => v,
        };
        match T::deserialize(v.clone()) {
            Ok(t) => Ok(Change::Set(t)),
            Err(e) => {
                // Older clients send the user's text, such as `Owner` or
                // `none`, and the server used to read it loosely.
                let serde_json::Value::String(s) = &v else {
                    return Err(serde::de::Error::custom(e));
                };
                let s = s.trim().to_ascii_lowercase();
                if s == "none" {
                    return Ok(Change::Clear);
                }
                T::deserialize(serde_json::Value::String(s))
                    .map(Change::Set)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}

impl<T: JsonSchema> JsonSchema for Change<T> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Change_{}", T::schema_name()).into()
    }

    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let set = g.subschema_for::<T>();
        json_schema!({
            "description": "Leave it out or send null to keep the value, send an empty string to clear it, or send a value to set it.",
            "anyOf": [set, { "const": "" }, { "type": "null" }]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WaitingOn;

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct U {
        #[serde(default, skip_serializing_if = "Change::is_keep")]
        a: Change<String>,
        #[serde(default, skip_serializing_if = "Change::is_keep")]
        w: Change<WaitingOn>,
    }

    fn read(j: &str) -> U {
        serde_json::from_str(j).unwrap()
    }

    #[test]
    fn wire_format() {
        assert_eq!(read("{}").a, Change::Keep);
        assert_eq!(read(r#"{"a":null}"#).a, Change::Keep);
        assert_eq!(read(r#"{"a":""}"#).a, Change::Clear);
        assert_eq!(read(r#"{"a":"me"}"#).a, Change::Set("me".into()));
        assert_eq!(read(r#"{"w":"owner"}"#).w, Change::Set(WaitingOn::Owner));
        assert_eq!(read(r#"{"w":" Owner"}"#).w, Change::Set(WaitingOn::Owner));
        assert_eq!(read(r#"{"w":"none"}"#).w, Change::Clear);
        assert!(serde_json::from_str::<U>(r#"{"w":"bob"}"#).is_err());
        let u = U {
            a: Change::Clear,
            w: Change::Set(WaitingOn::Agent),
        };
        assert_eq!(
            serde_json::to_string(&u).unwrap(),
            r#"{"a":"","w":"agent"}"#
        );
        assert_eq!(serde_json::to_string(&U::default()).unwrap(), "{}");
    }
}
