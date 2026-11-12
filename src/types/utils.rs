use serde::{Deserialize, Deserializer, de::IgnoredAny};

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
