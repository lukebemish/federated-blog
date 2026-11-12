use std::sync::Arc;

use activitypub_federation::{config::Data, fetch::object_id::ObjectId, kinds::activity::FollowType, traits::Activity};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{AppState, Error, types::objects::person::DbUser};

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Follow {
    pub(crate) actor: ObjectId<DbUser>,
    pub(crate) object: ObjectId<DbUser>,
    #[serde(rename = "type")]
    pub(crate) kind: FollowType,
    pub(crate) id: Url,
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
        let actor = self.actor.dereference(data).await?;
        let followed = self.object.dereference(data).await?;
        if followed.local.is_some() && actor.local.is_none() {
            sqlx::query("INSERT INTO followers (followee, follower) VALUES (?, ?) ON CONFLICT (followee, follower) DO NOTHING")
                .bind(followed.ap_id.inner().as_str())
                .bind(actor.ap_id.inner().as_str())
                .execute(&data.database).await?;
        }
        Ok(())
    }
}