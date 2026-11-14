use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, de::IgnoredAny};
use serde_json::{Number, Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DummyContext;

impl<'de> Deserialize<'de> for DummyContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        IgnoredAny::deserialize(deserializer)?;
        Ok(DummyContext)
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum StableValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<StableValue>),
    Object(BTreeMap<String, StableValue>)
}

impl Default for StableValue {
    fn default() -> Self {
        StableValue::Null
    }
}

impl Default for &StableValue {
    fn default() -> Self {
        const DEFAULT: StableValue = StableValue::Null;
        &DEFAULT
    }
}

impl From<StableValue> for Value {
    fn from(value: StableValue) -> Self {
        json!(value)
    }
}

impl From<Value> for StableValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(b),
            Value::String(s) => Self::String(s),
            Value::Number(n) => Self::Number(n),
            Value::Array(a) => Self::Array(a.into_iter().map(|v| v.into()).collect()),
            Value::Object(m) => Self::Object(m.into_iter().map(|(k, v)| (k, v.into())).collect())
        }
    }
}
