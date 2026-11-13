use activitypub_federation::kinds::collection::{OrderedCollectionPageType, OrderedCollectionType};
use serde::Serialize;
use url::Url;

use crate::Error;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", untagged)]
#[serde()]
pub(crate) enum LocalUserOwnedCollection {
    Root {
        id: Url,
        #[serde(rename = "type")]
        kind: OrderedCollectionType,
        total_items: usize,
        first: Url,
        last: Url
    },
    Page {
        id: Url,
        part_of: Url,
        #[serde(rename = "type")]
        kind: OrderedCollectionPageType,
        ordered_items: Vec<Url>,
        #[serde(skip_serializing_if = "Option::is_none")]
        next: Option<Url>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prev: Option<Url>,
    }
}

impl LocalUserOwnedCollection {
    pub(crate) async fn new<F>(id: Url, total_items: usize, page: Option<usize>, per_page: usize, items: F) -> Result<LocalUserOwnedCollection, Error>
        where F: AsyncFnOnce(usize, usize) -> Result<Vec<Url>, Error>
    {
        let last_idx = if total_items == 0 { 1 } else {
            (total_items - 1) / per_page + 1
        };
        Ok(match page {
            None => {
                let mut first = id.clone();
                let mut last = id.clone();

                let first_idx = 1;

                first.set_query(Some(&format!("page={}", first_idx)));
                last.set_query(Some(&format!("page={}", last_idx)));

                LocalUserOwnedCollection::Root {
                    id,
                    kind: OrderedCollectionType::OrderedCollection,
                    total_items,
                    first: first,
                    last: last
                }
            },
            Some(page) => {
                let mut next = id.clone();
                let mut prev = id.clone();
                let mut this = id.clone();

                next.set_query(Some(&format!("page={}", page+1)));
                prev.set_query(Some(&format!("page={}", page-1)));
                this.set_query(Some(&format!("page={}", page)));

                let first_idx = (page - 1) * per_page;
                let items: Vec<Url> = if first_idx >= total_items {
                    vec![]
                } else {
                    items(first_idx, per_page).await?
                };

                LocalUserOwnedCollection::Page {
                    id: this,
                    next: if page < last_idx { Some(next) } else { None },
                    prev: if page > 1 { Some(prev) } else { None },
                    part_of: id,
                    ordered_items: items,
                    kind: OrderedCollectionPageType::OrderedCollectionPage
                }
            }
        })
    }
}

