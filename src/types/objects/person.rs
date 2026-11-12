use crate::{AppState, Error, get_self_alias, types::{activities::Follow, utils::DummyContext}, with_slash};
use activitypub_federation::{
    config::Data, fetch::object_id::ObjectId, http_signatures::{Keypair, generate_actor_keypair}, kinds::actor::PersonType, protocol::{public_key::PublicKey, verification::verify_domains_match}, traits::{Activity, Actor, Object},
};
use anyhow::{anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use std::{collections::HashMap, fmt::Debug, sync::Arc};
use url::Url;

#[derive(Debug, Clone)]
pub(crate) struct DbUser {
    pub(crate) username: String,
    pub(crate) display_name: Option<String>,
    pub(crate) ap_id: ObjectId<DbUser>,
    pub(crate) inbox: Url,
    pub(crate) shared_inbox: Option<Url>,
    pub(crate) public_key: String,
    last_refreshed_at: DateTime<Utc>,
    pub(crate) extra: HashMap<String, Value>,
    pub(crate) local: Option<LocalDbUser>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalDbUser {
    pub(crate) private_key: String,
    pub(crate) outbox: Url,
    pub(crate) followers: Url
}

/// List of all activities which this actor can receive.
#[derive(Deserialize, Serialize, Debug)]
#[serde(untagged)]
#[enum_delegate::implement(Activity)]
pub(crate) enum PersonAcceptedActivities {
    Follow(Follow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalEndpoint {
    Outbox,
    Inbox,
    Followers
}

pub(crate) fn get_local_endpoint(state: &AppState, endpoint: LocalEndpoint) -> Result<Url, Error> {
    Ok(with_slash(&get_self_alias(state)?)?.join(match endpoint {
        LocalEndpoint::Outbox => "outbox",
        LocalEndpoint::Inbox => "inbox",
        LocalEndpoint::Followers => "followers"
    })?)
}

impl DbUser {
    async fn make_local(state: &Data<Arc<AppState>>) -> Result<DbUser, Error> {
        let ap_id = get_self_alias(state)?;

        let existing = DbUser::read_from_id(ap_id.clone(), state).await?;

        let existing_local = existing.and_then(|u| {
            u.local.clone().map(|l| (u, l))
        });

        let keypair = existing_local.as_ref().map(|u| {
            Ok(Keypair {
                private_key: u.1.private_key.clone(),
                public_key: u.0.public_key.clone()
            })
        }).unwrap_or_else(|| generate_actor_keypair())?;
        let local = LocalDbUser {
            private_key: keypair.private_key,
            outbox: get_local_endpoint(state, LocalEndpoint::Outbox)?,
            followers: get_local_endpoint(state, LocalEndpoint::Followers)?
        };
        let mut values: HashMap<String, Value> = HashMap::from([
            ("memorial".to_owned(), json!(false)),
            ("discoverable".to_owned(), json!(true)),
            ("manuallyApprovesFollowers".to_owned(), json!(false))
        ]);
        values.insert("url".to_string(), json!(state.config.homepage.to_string()));
        if let Some(icon) = state.config.icon.as_ref() {
            values.insert("icon".to_string(), icon.clone());
        }
        if let Some(image) = state.config.image.as_ref() {
            values.insert("image".to_string(), image.clone());
        }
        Ok(DbUser {
            username: state.config.username.to_string(),
            display_name: Some(state.config.name.clone()),
            ap_id: ap_id.into(),
            inbox: get_local_endpoint(state, LocalEndpoint::Inbox)?,
            shared_inbox: None,
            public_key: keypair.public_key,
            local: Some(local),
            last_refreshed_at: Utc::now(),
            extra: values
        })
    }

    pub(crate) async fn new_local(state: &Data<Arc<AppState>>) -> Result<DbUser, Error> {
        let user = DbUser::make_local(state).await?;
        let local = user.local.as_ref().unwrap(); // Safe since the other always has a local
        sqlx::query("INSERT INTO users (display_name, username, id, inbox, public_key, last_refreshed, extra) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET
            display_name = excluded.display_name,
            username = excluded.username,
            inbox = excluded.inbox,
            public_key = excluded.public_key,
            last_refreshed = excluded.last_refreshed,
            extra = excluded.extra")
            .bind(&user.display_name)
            .bind(&user.username)
            .bind(&user.ap_id.inner().as_str())
            .bind(&user.inbox.as_str())
            .bind(&user.public_key)
            .bind(&user.last_refreshed_at.timestamp_millis())
            .bind(serde_json::to_string(&user.extra)?)
            .execute(&state.database).await?;
        sqlx::query("INSERT INTO local_users (id, private_key) VALUES (?, ?) ON CONFLICT (id) DO UPDATE SET
            private_key = excluded.private_key")
            .bind(&user.ap_id.inner().as_str())
            .bind(&local.private_key)
            .execute(&state.database).await?;
        Ok(user)
    }

    pub(crate) async fn find_local(state: &Data<Arc<AppState>>, name: &str) -> Result<Option<DbUser>, Error> {
        if name == state.config.username {
            let maybe_id: Option<String> = sqlx::query_scalar("SELECT id FROM users u WHERE username = ? AND EXISTS ( SELECT 1 FROM local_users l_u WHERE l_u.id = u.id )")
                .bind(name)
                .fetch_optional(&state.database)
                .await?;
            match maybe_id {
                Some(id) => DbUser::read_from_id(Url::parse(&id)?, state).await,
                None => Ok(Some(DbUser::new_local(state).await?))
            }
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Person {
    #[serde(rename = "type")]
    kind: PersonType,
    preferred_username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    id: ObjectId<DbUser>,
    inbox: Url,
    #[serde(skip_serializing_if = "Option::is_none")]
    shared_inbox: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outbox: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    followers: Option<Url>,
    public_key: PublicKey,
    #[serde(skip_serializing, rename = "@context")]
    _context: DummyContext,
    #[serde(flatten)]
    extra: HashMap<String, Value>
}

#[async_trait::async_trait]
impl Object for DbUser {
    type DataType = Arc<AppState>;
    type Kind = Person;
    type Error = Error;

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        match self.local {
            None => Some(self.last_refreshed_at),
            Some(_) => None
        }
    }

    async fn read_from_id(
        object_id: Url,
        data: &Data<Self::DataType>,
    ) -> Result<Option<Self>, Self::Error> {
        Ok(match sqlx::query("SELECT display_name, username, inbox, public_key, shared_inbox, last_refreshed, extra FROM users WHERE id = ?")
            .bind(object_id.as_str())
            .fetch_optional(&data.database).await? {
            Some(row) => Some(
                DbUser {
                    display_name: row.get("display_name"),
                    username: row.get("username"),
                    inbox: Url::parse(row.get("inbox"))?,
                    public_key: row.get("public_key"),
                    shared_inbox: match row.get("shared_inbox") {
                        None => None,
                        Some(shared) => Some(Url::parse(shared)?)
                    },
                    last_refreshed_at: DateTime::from_timestamp_millis(row.get("last_refreshed")).ok_or(anyhow!("Invalid timestamp stored"))?,
                    local: match sqlx::query("SELECT private_key FROM local_users WHERE id = ?")
                        .bind(object_id.as_str())
                        .fetch_optional(&data.database).await? {
                        None => None,
                        Some(local_row) => Some(
                            LocalDbUser {
                                private_key: local_row.get("private_key"),
                                outbox: get_local_endpoint(data, LocalEndpoint::Outbox)?,
                                followers: get_local_endpoint(data, LocalEndpoint::Followers)?
                            }
                        )
                    },
                    extra: serde_json::from_str(row.get("extra"))?,
                    ap_id: object_id.into(),
                }
            ),
            None => None
        })
    }

    async fn into_json(self, _data: &Data<Self::DataType>) -> Result<Self::Kind, Self::Error> {
        Ok(Person {
            preferred_username: self.username.clone(),
            name: self.display_name.clone(),
            kind: Default::default(),
            id: self.ap_id.clone(),
            inbox: self.inbox.clone(),
            shared_inbox: self.shared_inbox.clone(),
            outbox: self.local.as_ref().map(|u| u.outbox.clone()),
            followers: self.local.as_ref().map(|u| u.followers.clone()),
            public_key: self.public_key(),
            _context: DummyContext,
            extra: self.extra.clone()
        })
    }

    async fn verify(
        json: &Self::Kind,
        expected_domain: &Url,
        _data: &Data<Self::DataType>,
    ) -> Result<(), Self::Error> {
        verify_domains_match(json.id.inner(), expected_domain)?;
        Ok(())
    }

    async fn from_json(
        json: Self::Kind,
        data: &Data<Self::DataType>,
    ) -> Result<Self, Self::Error> {
        let user = DbUser {
            display_name: json.name,
            username: json.preferred_username,
            ap_id: json.id,
            inbox: json.inbox,
            shared_inbox: json.shared_inbox,
            public_key: json.public_key.public_key_pem,
            local: None,
            last_refreshed_at: Utc::now(),
            extra: json.extra
        };
        sqlx::query("INSERT INTO users (display_name, username, id, inbox, shared_inbox, public_key, last_refreshed, extra) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET
            display_name = excluded.display_name,
            username = excluded.username,
            inbox = excluded.inbox,
            shared_inbox = excluded.shared_inbox,
            public_key = excluded.public_key,
            last_refreshed = excluded.last_refreshed,
            extra = excluded.extra")
            .bind(&user.display_name)
            .bind(&user.username)
            .bind(&user.ap_id.inner().as_str())
            .bind(&user.inbox.as_str())
            .bind(&user.shared_inbox.as_ref().map(|url| url.as_str()))
            .bind(&user.public_key)
            .bind(&user.last_refreshed_at.timestamp_millis())
            .bind(serde_json::to_string(&user.extra)?)
            .execute(&data.database).await?;
        Ok(user)
    }

    fn id(&self) -> &Url {
        self.ap_id.inner()
    }
}

impl Actor for DbUser {
    fn public_key_pem(&self) -> &str {
        &self.public_key
    }

    fn private_key_pem(&self) -> Option<String> {
        self.local.as_ref().map(|u| u.private_key.clone())
    }

    fn inbox(&self) -> Url {
        self.inbox.clone()
    }
}