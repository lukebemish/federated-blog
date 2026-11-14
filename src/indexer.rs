use std::sync::Arc;

use activitypub_federation::config::Data;
use serde::Deserialize;
use url::Url;

use crate::{AppState, Error, atproto::indexer::{session, update_article, update_profile}, get_self_alias, reqwest_client, types::{objects::article::DbLocalArticle, utils::StableValue}, with_slash};

#[derive(Deserialize, Debug)]
struct Post {
    id: String,
    rel: String,
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
    let mut articles = vec![];
    for post in posts.objects {
        let value: StableValue = reqwest_client().await.get(post.url).send()
            .await?
            .error_for_status()?
            .json().await?;
        articles.push((DbLocalArticle::new_local(state, value, self_alias_with_slash.join("objects/")?.join(&post.id)?).await?, post.rel));
    }

    if let Some(atproto) = &state.atproto_state {
        let auth = session(atproto).await?;
        let publication_rkey = update_profile(state, atproto, &auth).await?;
        for (article, id_string) in articles {
            update_article(state, atproto, &auth, &id_string, &article, &publication_rkey).await?;
        }
    }

    Ok(())
}
