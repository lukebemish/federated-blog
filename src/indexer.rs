use std::sync::Arc;

use activitypub_federation::config::Data;
use serde::Deserialize;
use url::Url;

use crate::{AppState, Error, atproto::indexer::update_profile, get_self_alias, reqwest_client, types::{objects::article::DbLocalArticle, utils::StableValue}, with_slash};

#[derive(Deserialize, Debug)]
struct Post {
    id: String,
    url: Url
}

#[derive(Deserialize, Debug)]
struct Posts {
    objects: Vec<Post>
}

pub(crate) async fn run_index(state: &Data<Arc<AppState>>) -> Result<(), Error> {
    let self_alias_with_slash: Url = with_slash(&get_self_alias(state)?)?;
    let posts: Posts = reqwest_client().await.get(state.config.posts.as_str()).send()
        .await?
        .error_for_status()?
        .json().await?;
    for post in posts.objects {
        let value: StableValue = reqwest_client().await.get(post.url).send()
            .await?
            .error_for_status()?
            .json().await?;
        DbLocalArticle::new_local(state, value, self_alias_with_slash.join("objects/")?.join(&post.id)?).await?;
    }

    if let Some(atproto) = &state.atproto_state {
        update_profile(state, atproto).await?;
    }

    Ok(())
}
