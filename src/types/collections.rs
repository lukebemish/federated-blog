use activitypub_federation::kinds::collection::OrderedCollectionType;
use serde::Serialize;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(bound = "")]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalUserOwnedCollection {
    pub(crate) id: Url,
    #[serde(rename = "type")]
    pub(crate) kind: OrderedCollectionType, // Always "OrderedCollection"
    pub(crate) total_items: usize,
    pub(crate) ordered_items: Vec<Url>
}
