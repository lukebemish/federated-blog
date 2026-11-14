use std::{collections::HashMap, sync::Arc};

use activitypub_federation::{config::Data, fetch::object_id::ObjectId, kinds::object::ArticleType, protocol::verification::verify_domains_match, traits::Object};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use url::Url;

use crate::{AppState, Error, get_self_alias, types::{activities::Outbound, objects::person::{DbUser, LocalEndpoint, get_local_endpoint}, utils::{DummyContext, StableValue}}};

#[derive(Debug, Clone)]
pub(crate) struct DbLocalArticle {
    pub(crate) name: String,
    pub(crate) ap_id: ObjectId<DbLocalArticle>,
    pub(crate) attributed_to: ObjectId<DbUser>,
    pub(crate) url: Url,
    pub(crate) to: Vec<Url>,
    pub(crate) cc: Vec<Url>,
    pub(crate) extra: HashMap<String, StableValue>,
}

impl DbLocalArticle {
    pub(crate) async fn new_local(data: &Data<Arc<AppState>>, mut value: StableValue, ap_id: Url) -> Result<DbLocalArticle, Error> {
        match &mut value {
            StableValue::Object(map) => {
                map.insert("to".to_owned(), StableValue::Array(vec![
                    StableValue::String("https://www.w3.org/ns/activitystreams#Public".to_owned())
                ]));
                map.insert("cc".to_owned(), StableValue::Array(vec![
                    StableValue::String(get_local_endpoint(data, LocalEndpoint::Followers)?.to_string())
                ]));
            }
            _ => {}
        }
        let value = value; // no more mut

        let existing = DbLocalArticle::read_from_id(ap_id.clone(), data).await?;
        match existing {
            None => {
                let article = DbLocalArticle::from_json(serde_json::from_value(value.into())?, data).await?;
                Outbound::create_article(
                    data,
                    &article.attributed_to,
                    article.clone().into_json(data).await?
                ).await?;
                log::info!("Indexed creation of {ap_id}");
                Ok(article)
            },
            Some(old) => {
                let old_json = DbLocalArticle::into_json(old.clone(), data).await?;
                let new_json: LocalArticle = serde_json::from_value(value.into())?;
                if old_json != new_json {
                    let article = DbLocalArticle::from_json(new_json, data).await?;
                    Outbound::modify_article(
                        data,
                        &article.attributed_to,
                        article.clone().into_json(data).await?
                    ).await?;
                    log::info!("Indexed modification of {ap_id}");
                    Ok(article)
                } else {
                    Ok(old)
                }
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalArticle {
    #[serde(rename = "type")]
    kind: ArticleType,
    name: String,
    id: ObjectId<DbLocalArticle>,
    attributed_to: ObjectId<DbUser>,
    url: Url,
    #[serde(default)]
    pub(crate) to: Vec<Url>,
    #[serde(default)]
    pub(crate) cc: Vec<Url>,
    #[serde(skip_serializing, default, rename = "@context")]
    _context: DummyContext,
    #[serde(flatten)]
    extra: HashMap<String, StableValue>
}

#[async_trait::async_trait]
impl Object for DbLocalArticle {
    type DataType = Arc<AppState>;
    type Kind = LocalArticle;
    type Error = Error;

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        None
    }

    async fn verify(
        json: &Self::Kind,
        expected_domain: &Url,
        data: &Data<Self::DataType>,
    ) -> Result<(), Self::Error> {
        verify_domains_match(json.id.inner(), expected_domain)?;
        // This is purely for _local_ articles -- e.g., from me!
        let my_domain = get_self_alias(&data)?;
        verify_domains_match(json.id.inner(), &my_domain)?;
        Ok(())
    }

    async fn into_json(self, _data: &Data<Self::DataType>) -> Result<Self::Kind, Self::Error> {
        Ok(LocalArticle {
            kind: Default::default(),
            name: self.name.clone(),
            id: self.ap_id.clone(),
            attributed_to: self.attributed_to.clone(),
            url: self.url.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            _context: DummyContext,
            extra: self.extra.clone()
        })
    }

    async fn read_from_id(
        object_id: Url,
        data: &Data<Self::DataType>,
    ) -> Result<Option<Self>, Self::Error> {
        Ok(match sqlx::query("SELECT author, url, to_url, cc, extra, name FROM blog_articles WHERE id = ?")
            .bind(object_id.as_str())
            .fetch_optional(&data.database).await? {
                None => None,
                Some(row) => Some(DbLocalArticle {
                    name: row.get("name"),
                    ap_id: object_id.into(),
                    attributed_to: Url::parse(row.get("author"))?.into(),
                    url: Url::parse(row.get("url"))?,
                    to: serde_json::from_str(row.get("to_url"))?,
                    cc: serde_json::from_str(row.get("cc"))?,
                    extra: serde_json::from_str(row.get("extra"))?
                })
            }
        )
    }

    async fn from_json(
        json: Self::Kind,
        data: &Data<Self::DataType>,
    ) -> Result<Self, Self::Error> {
        let local_article = DbLocalArticle {
            name: json.name,
            ap_id: json.id,
            attributed_to: json.attributed_to,
            url: json.url,
            to: json.to,
            cc: json.cc,
            extra: json.extra
        };
        sqlx::query("INSERT INTO blog_articles (id, author, url, to_url, cc, extra, name) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET
            author = excluded.author,
            url = excluded.url,
            to_url = excluded.to_url,
            cc = excluded.cc,
            extra = excluded.extra,
            name = excluded.name")
            .bind(&local_article.ap_id.inner().as_str())
            .bind(&local_article.attributed_to.inner().as_str())
            .bind(&local_article.url.as_str())
            .bind(serde_json::to_string(&local_article.to)?)
            .bind(serde_json::to_string(&local_article.cc)?)
            .bind(serde_json::to_string(&local_article.extra)?)
            .bind(&local_article.name)
            .execute(&data.database).await?;
        Ok(local_article)
    }

    fn id(&self) -> &Url {
        self.ap_id.inner()
    }
}

