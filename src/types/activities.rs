use std::{collections::{BTreeMap}, sync::Arc};

use activitypub_federation::{activity_queue::queue_activity, config::Data, fetch::object_id::ObjectId, kinds::activity::{AcceptType, CreateType, FollowType, UndoType, UpdateType}, traits::{Activity, Actor}};
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use serde::{Deserialize, Serialize, Serializer, ser::{SerializeMap, SerializeSeq}};
use serde_json::Value;
use sqlx::Row;
use url::Url;

use crate::{AppState, Error, types::{activities::Outbound::{AcceptFollow, CreateArticle, ModifyArticle}, objects::{article::LocalArticle, person::DbUser}}};

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Follow {
    pub(crate) actor: ObjectId<DbUser>,
    pub(crate) object: ObjectId<DbUser>,
    #[serde(rename = "type")]
    pub(crate) kind: FollowType,
    pub(crate) id: Url,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>
}

#[async_trait]
impl Activity for Follow {
    type DataType = Arc<AppState>;
    type Error = Error;

    fn id(&self) -> &Url {
        &self.id
    }

    fn actor(&self) -> &Url {
        self.actor.inner()
    }
    
    async fn verify(&self,  _data: &Data<Self::DataType>) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn receive(self, data: &Data<Self::DataType>) -> Result<(), Self::Error> {
        let clone = self.clone();
        let actor = self.actor.dereference(data).await?;
        let followed = self.object.dereference(data).await?;
        if followed.local.is_some() && actor.local.is_none() {
            sqlx::query("INSERT INTO followers (followee, follower) VALUES (?, ?) ON CONFLICT (followee, follower) DO NOTHING")
                .bind(followed.ap_id.inner().as_str())
                .bind(actor.ap_id.inner().as_str())
                .execute(&data.database).await?;
        } else {
            return Err(Error::Simple(anyhow!(
                format!("Could not follow: actor {} trying to follow target {}", actor.ap_id.inner(), followed.ap_id.inner()
            ))));
        }
        Outbound::accept_follow(data, &followed.ap_id, clone).await?;
        log::info!("Accepted follow from {}", actor.ap_id.inner());
        Ok(())
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UndoFollow {
    pub(crate) actor: ObjectId<DbUser>,
    pub(crate) object: Follow,
    #[serde(rename = "type")]
    pub(crate) kind: UndoType,
    pub(crate) id: Url,
}

#[async_trait]
impl Activity for UndoFollow {
    type DataType = Arc<AppState>;
    type Error = Error;

    fn id(&self) -> &Url {
        &self.id
    }

    fn actor(&self) -> &Url {
        self.actor.inner()
    }
    
    async fn verify(&self,  _data: &Data<Self::DataType>) -> Result<(), Self::Error> {
        if self.actor != self.object.actor {
            return Err(Error::Simple(anyhow!("Un-follow requests must be made by the actor that initiated the follow!")));
        }
        Ok(())
    }

    async fn receive(self, data: &Data<Self::DataType>) -> Result<(), Self::Error> {
        let actor = self.actor.dereference(data).await?;
        let followed = self.object.object.dereference(data).await?;
        if followed.local.is_some() && actor.local.is_none() {
            sqlx::query("DELETE FROM followers WHERE followee = ? AND follower = ?")
                .bind(followed.ap_id.inner().as_str())
                .bind(actor.ap_id.inner().as_str())
                .execute(&data.database).await?;
        } else {
            return Err(Error::Simple(anyhow!(
                format!("Could not un-follow: actor {} trying to un-follow target {}", actor.ap_id.inner(), followed.ap_id.inner()
            ))));
        }
        log::info!("Undid follow from {}", actor.ap_id.inner());
        Ok(())
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnknownActivity {
    pub(crate) actor: ObjectId<DbUser>,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) id: Url,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>
}

#[async_trait]
impl Activity for UnknownActivity {
    type DataType = Arc<AppState>;
    type Error = Error;

    fn id(&self) -> &Url {
        &self.id
    }

    fn actor(&self) -> &Url {
        self.actor.inner()
    }
    
    async fn verify(&self,  _data: &Data<Self::DataType>) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn receive(self, _: &Data<Self::DataType>) -> Result<(), Self::Error> {
        log::info!("No-op for unknown activity {} from {}", serde_json::to_string(&self)?, self.actor.inner());
        Ok(())
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase", untagged)]
pub(crate) enum Outbound {
    CreateArticle {
        #[serde(rename = "type")]
        kind: CreateType,
        #[serde(skip_serializing)]
        id_part: String,
        id: Url,
        actor: ObjectId<DbUser>,
        published: DateTime<Utc>,
        #[serde(serialize_with = "serialize_copy_object_recipients", flatten)]
        object: LocalArticle,
    },
    ModifyArticle {
        #[serde(rename = "type")]
        kind: UpdateType,
        #[serde(skip_serializing)]
        id_part: String,
        id: Url,
        actor: ObjectId<DbUser>,
        published: DateTime<Utc>,
        #[serde(serialize_with = "serialize_copy_object_recipients", flatten)]
        object: LocalArticle,
    },
    AcceptFollow {
        #[serde(rename = "type")]
        kind: AcceptType,
        #[serde(skip_serializing)]
        id_part: String,
        id: Url,
        actor: ObjectId<DbUser>,
        published: DateTime<Utc>,
        object: Follow,
        #[serde(rename = "to", serialize_with = "singleton_list")]
        target_inbox: Url
    },
}

fn singleton_list<D, S>(field: &D, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer, D: Serialize
{
    let mut seq = serializer.serialize_seq(Some(1))?;
    seq.serialize_element(field)?;
    seq.end()
}

fn serialize_copy_object_recipients<S>(field: &LocalArticle, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer
{
    let mut map = serializer.serialize_map(Some(3))?;
    map.serialize_entry("object", field)?;
    map.serialize_entry("to", &field.to)?;
    map.serialize_entry("cc", &field.cc)?;
    map.end()
}

#[async_trait]
impl Activity for Outbound {
    type DataType = Arc<AppState>;
    type Error = Error;

    fn id(&self) -> &Url {
        match self {
            CreateArticle { id, .. } => id,
            ModifyArticle { id, .. } => id,
            AcceptFollow { id, .. } => id,
        }
    }

    fn actor(&self) -> &Url {
        match self {
            CreateArticle { actor, .. } => actor.inner(),
            ModifyArticle { actor, .. } => actor.inner(),
            AcceptFollow { actor, .. } => actor.inner(),
        }
    }
    
    async fn verify(&self,  _data: &Data<Self::DataType>) -> Result<(), Self::Error> {
        Err(Error::Simple(anyhow!("Cannot receive outbound activities")))
    }

    async fn receive(self, _data: &Data<Self::DataType>) -> Result<(), Self::Error> {
        Err(Error::Simple(anyhow!("Cannot receive outbound activities")))
    }
}

impl Outbound {
    fn actor_id(&self) -> &ObjectId<DbUser> {
        match self {
            CreateArticle { actor, .. } => actor,
            ModifyArticle { actor, .. } => actor,
            AcceptFollow { actor, .. } => actor,
        }
    }

    pub(crate) async fn create_article(state: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>, object: LocalArticle) -> Result<(), Error> {
        let id_part = new_activity_id();
        let id = get_activities_root(state)?.join(&id_part)?;
        let activity = CreateArticle {
            kind: CreateType::Create,
            id_part,
            id,
            actor: actor.clone(),
            published: Utc::now(),
            object
        };
        submit_outbound(state, activity).await?;
        Ok(())
    }

    pub(crate) async fn modify_article(state: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>, object: LocalArticle) -> Result<(), Error> {
        let id_part = new_activity_id();
        let id = get_activities_root(state)?.join(&id_part)?;
        let activity = ModifyArticle {
            kind: UpdateType::Update,
            id_part,
            id,
            actor: actor.clone(),
            published: Utc::now(),
            object
        };
        submit_outbound(state, activity).await?;
        Ok(())
    }

    pub(crate) async fn accept_follow(state: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>, follow: Follow) -> Result<(), Error> {
        let id_part = new_activity_id();
        let id = get_activities_root(state)?.join(&id_part)?;
        let activity = AcceptFollow {
            kind: AcceptType::Accept,
            id_part,
            id,
            actor: actor.clone(),
            published: Utc::now(),
            target_inbox: follow.actor.clone().dereference(state).await?.inbox,
            object: follow
        };
        submit_outbound(state, activity).await?;
        Ok(())
    }
}

pub(crate) fn get_activities_root(state: &AppState) -> Result<Url, Error> {
    Ok(state.config.host.url.join("activities/")?)
}

pub(crate) fn new_activity_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) async fn submit_outbound(data: &Data<Arc<AppState>>, outbound: Outbound) -> Result<(), Error> {
    match &outbound {
        Outbound::ModifyArticle {
            kind: _,
            id_part,
            actor,
            object,
            published,
            id: _
        } => sqlx::query("INSERT INTO outbound_activities (id_part, actor, kind, object, published) VALUES (?, ?, ?, ?, ?)")
            .bind(id_part)
            .bind(actor.inner().as_str())
            .bind("Update")
            .bind(serde_json::to_string(&object)?)
            .bind(published.timestamp_millis()),
        Outbound::CreateArticle {
            kind: _,
            id_part,
            actor,
            object,
            published,
            id: _
        } => sqlx::query("INSERT INTO outbound_activities (id_part, actor, kind, object, published) VALUES (?, ?, ?, ?, ?)")
            .bind(id_part)
            .bind(actor.inner().as_str())
            .bind("Create")
            .bind(serde_json::to_string(&object)?)
            .bind(published.timestamp_millis()),
        Outbound::AcceptFollow {
            kind: _,
            id_part,
            actor,
            object,
            published,
            target_inbox: _,
            id: _
        } => sqlx::query("INSERT INTO outbound_activities (id_part, actor, kind, object, published) VALUES (?, ?, ?, ?, ?)")
            .bind(id_part)
            .bind(actor.inner().as_str())
            .bind("Accept(Follow)")
            .bind(serde_json::to_string(&object)?)
            .bind(published.timestamp_millis()),
    }.execute(&data.database).await?;

    let inboxes = match &outbound {
        CreateArticle { .. } => get_inboxes(data, get_followers(data, outbound.actor_id()).await?).await?,
        ModifyArticle { .. } => get_inboxes(data, get_followers(data, outbound.actor_id()).await?).await?,
        AcceptFollow { object: follow, .. } => vec![
            follow.actor.clone().dereference(data).await?.inbox()
        ],
    };

    let actor = outbound.actor_id().dereference_local(&data).await?;

    if let None = actor.local {
        return Err(Error::Simple(anyhow!("Cannot send activity for non-local actor {}", actor.ap_id.inner().as_str())))
    }

    queue_activity(&outbound, &actor, inboxes, &data).await?;

    Ok(())
}

async fn get_inboxes(data: &Data<Arc<AppState>>, actors: Vec<ObjectId<DbUser>>) -> Result<Vec<Url>, Error> {
    join_all(actors.iter().map(|follower| async {
        let actor = follower.dereference(data).await?;
        Ok::<Url, Error>(actor.shared_inbox_or_inbox())
    })).await.into_iter().collect::<Result<Vec<Url>, Error>>()
}

pub(crate) async fn count_followers(data: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>) -> Result<usize, Error> {
    let count: u64 = sqlx::query_scalar("SELECT COUNT(*) FROM followers WHERE followee = ?")
        .bind(actor.inner().as_str())
        .fetch_one(&data.database)
        .await?;
    Ok(count.try_into()?)
}

pub(crate) async fn get_followers(data: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>) -> Result<Vec<ObjectId<DbUser>>, Error> {
    let followers: Vec<String> = sqlx::query_scalar("SELECT follower FROM followers WHERE followee = ?")
        .bind(actor.inner().as_str())
        .fetch_all(&data.database)
        .await?;
    followers.iter().map(|follower| {
        let id = Url::parse(follower)?;
        Ok(ObjectId::<DbUser>::from(id))
    }).collect()
}

pub(crate) async fn get_followers_from(data: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>, start: usize, limit: usize) -> Result<Vec<ObjectId<DbUser>>, Error> {
    let limit: u32 = limit.try_into()?;
    let start: u32 = start.try_into()?;
    let followers: Vec<String> = sqlx::query_scalar("SELECT follower FROM followers WHERE followee = ? LIMIT ? OFFSET ?")
        .bind(actor.inner().as_str())
        .bind(limit)
        .bind(start)
        .fetch_all(&data.database)
        .await?;
    followers.iter().map(|follower| {
        let id = Url::parse(follower)?;
        Ok(ObjectId::<DbUser>::from(id))
    }).collect()
}

pub(crate) async fn get_outbound(data: &Data<Arc<AppState>>, id_part: &str) -> Result<Option<Outbound>, Error> {
    let row = sqlx::query("SELECT actor, kind, object, published FROM outbound_activities WHERE id_part = ?")
        .bind(id_part)
        .fetch_optional(&data.database)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => {
            let actor: ObjectId<DbUser> = Url::parse(row.get("actor"))?.into();
            let kind: String = row.get("kind");
            let object: Option<String> = row.get("object");
            let full_url = get_activities_root(data)?.join(id_part)?;
            let published = DateTime::from_timestamp_millis(row.get("published")).ok_or_else(|| anyhow!("not a valid date!"))?;
            match (kind.as_str(), object) {
                ("Create", Some(object)) => Ok(Some(Outbound::CreateArticle {
                    kind: CreateType::Create,
                    id_part: id_part.to_owned(),
                    id: full_url,
                    actor,
                    published,
                    object: serde_json::from_str(&object)?
                })),
                ("Update", Some(object)) => Ok(Some(Outbound::ModifyArticle {
                    kind: UpdateType::Update,
                    id_part: id_part.to_owned(),
                    id: full_url,
                    actor,
                    published,
                    object: serde_json::from_str(&object)?
                })),
                ("Accept(Follow)", Some(object)) => {
                    let follow: Follow = serde_json::from_str(&object)?;
                    Ok(Some(Outbound::AcceptFollow {
                        kind: AcceptType::Accept,
                        id_part: id_part.to_owned(),
                        id: full_url,
                        actor,
                        published,
                        target_inbox: follow.actor.clone().dereference(data).await?.inbox,
                        object: follow
                    }))
                },
                _ => Ok(None)
            }
        }
    }
}

pub(crate) async fn count_outbox_for(data: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>) -> Result<usize, Error> {
    let count: u64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_activities WHERE actor = ? AND kind IN ('Create', 'Update')")
        .bind(actor.inner().as_str())
        .fetch_one(&data.database)
        .await?;
    Ok(count.try_into()?)
}

pub(crate) async fn get_outbox_for(data: &Data<Arc<AppState>>, actor: &ObjectId<DbUser>, start: usize, limit: usize) -> Result<Vec<Url>, Error> {
    let limit: u32 = limit.try_into()?;
    let start: u32 = start.try_into()?;
    let rows: Vec<String> = sqlx::query_scalar("SELECT id_part FROM outbound_activities WHERE actor = ? AND kind IN ('Create', 'Update') ORDER BY published DESC LIMIT ? OFFSET ?")
        .bind(actor.inner().as_str())
        .bind(limit)
        .bind(start)
        .fetch_all(&data.database)
        .await?;
    rows.into_iter().map(|row| {
        Ok(get_activities_root(data)?.join(&row)?)
    }).collect()
}