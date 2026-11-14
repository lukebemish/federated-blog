use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeMap};
use url::Url;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct SiteStandardPublicationType();
impl Serialize for SiteStandardPublicationType { fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str("site.standard.publication")
}}
impl TryFrom<String> for SiteStandardPublicationType { type Error = String; fn try_from(value: String) -> Result<Self, Self::Error> {
    const EXPECT: &str = "site.standard.publication";
    if value == EXPECT { Ok(Default::default()) } else { Err(format!("Expected {}", EXPECT)) }
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SiteStandardPublication {
    #[serde(rename = "$type")]
    pub(crate) kind: SiteStandardPublicationType,
    pub(crate) url: Url,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preferences: Option<Preferences>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<Blob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) basic_theme: Option<BasicTheme>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) show_in_discover: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct BasicThemeType();
impl Serialize for BasicThemeType { fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str("site.standard.basic.theme")
}}
impl TryFrom<String> for BasicThemeType { type Error = String; fn try_from(value: String) -> Result<Self, Self::Error> {
    const EXPECT: &str = "site.standard.basic.theme";
    if value == EXPECT { Ok(Default::default()) } else { Err(format!("Expected {}", EXPECT)) }
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BasicTheme {
    #[serde(rename = "$type")]
    pub(crate) kind: BasicThemeType,
    pub(crate) background: BasicThemeRgb,
    pub(crate) foreground: BasicThemeRgb,
    pub(crate) accent: BasicThemeRgb,
    pub(crate) accent_foreground: BasicThemeRgb,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct BasicThemeRgbType();
impl Serialize for BasicThemeRgbType { fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str("site.standard.basic.theme#rgb")
}}
impl TryFrom<String> for BasicThemeRgbType { type Error = String; fn try_from(value: String) -> Result<Self, Self::Error> {
    const EXPECT: &str = "site.standard.basic.theme#rgb";
    if value == EXPECT { Ok(Default::default()) } else { Err(format!("Expected {}", EXPECT)) }
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BasicThemeRgb {
    #[serde(rename = "$type")]
    pub(crate) kind: BasicThemeRgbType,
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct BlobType();
impl Serialize for BlobType { fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str("blob")
}}
impl TryFrom<String> for BlobType { type Error = String; fn try_from(value: String) -> Result<Self, Self::Error> {
    const EXPECT: &str = "blob";
    if value == EXPECT { Ok(Default::default()) } else { Err(format!("Expected {}", EXPECT)) }
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Blob {
    #[serde(rename = "$type")]
    pub(crate) kind: BlobType,
    pub(crate) mime_type: String,
    pub(crate) size: u32,
    #[serde(serialize_with = "ref_as_link", deserialize_with = "link_as_ref", rename = "ref")]
    pub(crate) blob_ref: String,
}

fn ref_as_link<S>(s: &String, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer
{
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry("$link", &s)?;
    map.end()
}

fn link_as_ref<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>
{
    #[derive(Deserialize)]
    struct BlobLink {
        #[serde(rename = "$link")]
        link: String
    }

    BlobLink::deserialize(deserializer).map(|it| it.link)
}
