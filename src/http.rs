use std::sync::Arc;

use activitypub_federation::{axum::{inbox::{ActivityData, receive_activity}, json::FederationJson}, config::Data, fetch::webfinger::{Webfinger, build_webfinger_response, extract_webfinger_name}, kinds::collection::OrderedCollectionType::OrderedCollection, protocol::context::WithContext, traits::Object};
use axum::{Json, debug_handler, extract::{Path, Query}, response::{IntoResponse, Response}};
use log::error;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::{AppState, Error, default_context, get_self_alias, types::{activities::{get_followers, get_outbound, get_outbound_for}, collections::LocalUserOwnedCollection, objects::{article::{DbLocalArticle, LocalArticle}, person::{DbUser, Person, PersonAcceptedActivities}}}, with_slash};

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Error::Simple(e) => {
                error!("{}", e);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            },
            Error::Status(s) => s.into_response()
        }
    }
}

#[debug_handler]
pub(crate) async fn http_get_user(
    Path(name): Path<String>,
    data: Data<Arc<AppState>>,
) -> Result<FederationJson<WithContext<Person>>, Error> {
    let db_user = DbUser::find_local(&data, &name).await?;
    match db_user {
        None => Err(Error::Status(StatusCode::NOT_FOUND)),
        Some(user) => {
            let json_user = user.into_json(&data).await?;
            Ok(FederationJson(WithContext::new(json_user, default_context())))
        }
    }
}

#[debug_handler]
pub(crate) async fn http_get_article(
    Path((_, path)): Path<(String, String)>,
    data: Data<Arc<AppState>>,
) -> Result<FederationJson<WithContext<LocalArticle>>, Error> {
    let db_local_article = DbLocalArticle::read_from_id(
        with_slash(&get_self_alias(&data)?)?.join("objects/")?.join(&path)?,
        &data
    ).await?;
    match db_local_article {
        None => Err(Error::Status(StatusCode::NOT_FOUND)),
        Some(local_article) => {
            let json_local_article = local_article.into_json(&data).await?;
            Ok(FederationJson(WithContext::new(json_local_article, default_context())))
        }
    }
}

#[debug_handler]
pub(crate) async fn http_post_user_inbox(
    data: Data<Arc<AppState>>,
    activity_data: ActivityData,
) -> impl IntoResponse {
    receive_activity::<WithContext<PersonAcceptedActivities>, DbUser, Arc<AppState>>(
        activity_data,
        &data,
    )
    .await
}

#[debug_handler]
pub(crate) async fn http_get_user_outbox(
    Path(name): Path<String>,
    data: Data<Arc<AppState>>
) -> impl IntoResponse {
    let user = DbUser::find_local(&data, &name).await?;
    if let Some(user) = user && let Some(local_user) = &user.local {
        let outbound = get_outbound_for(&data, &user.ap_id).await?;
        let outbox = LocalUserOwnedCollection {
            id: local_user.outbox.clone(),
            kind: OrderedCollection,
            total_items: outbound.len(),
            ordered_items: outbound
        };
        Ok(FederationJson(WithContext::new(outbox, default_context())))
    } else {
        Err(Error::Status(StatusCode::NOT_FOUND))
    }
}

#[debug_handler]
pub(crate) async fn http_get_user_followers(
    Path(name): Path<String>,
    data: Data<Arc<AppState>>
) -> impl IntoResponse {
    let user = DbUser::find_local(&data, &name).await?;
    if let Some(user) = user && let Some(local_user) = &user.local {
        let followers = get_followers(&data, &user.ap_id).await?;
        let outbox = LocalUserOwnedCollection {
            id: local_user.followers.clone(),
            kind: OrderedCollection,
            total_items: followers.len(),
            ordered_items: followers.into_iter().map(|id| id.into_inner()).collect()
        };
        Ok(FederationJson(WithContext::new(outbox, default_context())))
    } else {
        Err(Error::Status(StatusCode::NOT_FOUND))
    }
}

#[debug_handler]
pub(crate) async fn http_get_outbound_activity(
    Path(id_part): Path<String>,
    data: Data<Arc<AppState>>
) -> impl IntoResponse {
    let outbound = get_outbound(&data, &id_part).await?;
    match outbound {
        None => Err(Error::Status(StatusCode::NOT_FOUND)),
        Some(activity) => Ok(FederationJson(WithContext::new(activity, default_context())))
    }
}

#[derive(Deserialize)]
pub(crate) struct WebfingerQuery {
    resource: String,
}

#[debug_handler]
pub(crate) async fn webfinger(
    Query(query): Query<WebfingerQuery>,
    data: Data<Arc<AppState>>,
) -> Result<Json<Webfinger>, Error> {
    let name = extract_webfinger_name(&query.resource, &data)?;
    let db_user = DbUser::find_local(&data, name).await?;
    match db_user {
        None => Err(Error::Status(StatusCode::NOT_FOUND)),
        Some(user) => Ok(Json(build_webfinger_response(
            query.resource,
            user.ap_id.into_inner(),
        )))
    }
}
