use std::sync::Arc;

use activitypub_federation::{axum::{inbox::{ActivityData, receive_activity}, json::FederationJson}, config::Data, fetch::webfinger::{Webfinger, build_webfinger_response, extract_webfinger_name}, protocol::context::WithContext, traits::Object};
use axum::{Json, debug_handler, extract::{Path, Query}, http::HeaderMap, response::{IntoResponse, Redirect, Response}};
use log::error;
use reqwest::{StatusCode, header::ACCEPT};
use serde::Deserialize;
use serde_json::Value;

use crate::{AppState, Error, default_context, get_self_alias, types::{activities::{count_followers, count_outbox_for, get_followers_from, get_outbound, get_outbox_for}, collections::LocalUserOwnedCollection, objects::{article::DbLocalArticle, person::{DbUser, PersonAcceptedActivities}}}, with_slash};

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

fn should_serve_html(headers: HeaderMap) -> bool {
    if let Some(accept) = headers.get(ACCEPT) {
        if let Ok(accept_str) = accept.to_str() {
            if accept_str.contains("text/html") && !accept_str.contains("application/activity+json") && !accept_str.contains("application/ld+json") {
                return true;
            }
        }
    }
    false
}

#[debug_handler]
pub(crate) async fn http_get_user(
    Path(name): Path<String>,
    data: Data<Arc<AppState>>,
    headers: HeaderMap
) -> Result<Response, Error> {
    let db_user = DbUser::find_local(&data, &name).await?;
    match db_user {
        None => Err(Error::Status(StatusCode::NOT_FOUND)),
        Some(user) => {
            if let Some(Value::String(url)) = user.extra.get("url") && should_serve_html(headers) {
                return Ok(Redirect::to(url).into_response());
            }
            let json_user = user.into_json(&data).await?;
            Ok(FederationJson(WithContext::new(json_user, default_context())).into_response())
        }
    }
}

#[debug_handler]
pub(crate) async fn http_get_article(
    Path((_, path)): Path<(String, String)>,
    data: Data<Arc<AppState>>,
    headers: HeaderMap
) -> Result<Response, Error> {
    let db_local_article = DbLocalArticle::read_from_id(
        with_slash(&get_self_alias(&data)?)?.join("objects/")?.join(&path)?,
        &data
    ).await?;
    match db_local_article {
        None => Err(Error::Status(StatusCode::NOT_FOUND)),
        Some(local_article) => {
            if should_serve_html(headers) {
                return Ok(Redirect::to(local_article.url.as_str()).into_response());
            }
            let json_local_article = local_article.into_json(&data).await?;
            Ok(FederationJson(WithContext::new(json_local_article, default_context())).into_response())
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
    .await.map_err(|e| {
        log::error!("{}", e);
        StatusCode::UNPROCESSABLE_ENTITY
    })
}

#[derive(Deserialize)]
pub(crate) struct Pagination {
    page: Option<usize>,
}

#[debug_handler]
pub(crate) async fn http_get_user_outbox(
    Path(name): Path<String>,
    data: Data<Arc<AppState>>,
    Query(pagination): Query<Pagination>
) -> impl IntoResponse {
    let user = DbUser::find_local(&data, &name).await?;
    if let Some(user) = user && let Some(local_user) = &user.local {
        let outbox = LocalUserOwnedCollection::new(
            local_user.outbox.clone(),
            count_outbox_for(&data, &user.ap_id).await?,
            pagination.page,
            10,
            async |start, limit| get_outbox_for(&data, &user.ap_id, start, limit).await
        ).await?;
        Ok(FederationJson(WithContext::new(outbox, default_context())))
    } else {
        Err(Error::Status(StatusCode::NOT_FOUND))
    }
}

#[debug_handler]
pub(crate) async fn http_get_user_followers(
    Path(name): Path<String>,
    data: Data<Arc<AppState>>,
    Query(pagination): Query<Pagination>
) -> impl IntoResponse {
    let user = DbUser::find_local(&data, &name).await?;
    if let Some(user) = user && let Some(local_user) = &user.local {
        let outbox = LocalUserOwnedCollection::new(
            local_user.followers.clone(),
            count_followers(&data, &user.ap_id).await?,
            pagination.page,
            10,
            async |start, limit| Ok(get_followers_from(&data, &user.ap_id, start, limit).await?
                .into_iter().map(|id| id.into_inner()).collect())
        ).await?;
        Ok(FederationJson(WithContext::new(outbox, default_context())))
    } else {
        Err(Error::Status(StatusCode::NOT_FOUND))
    }
}

#[debug_handler]
pub(crate) async fn http_get_user_following(
    Path(name): Path<String>,
    data: Data<Arc<AppState>>,
    Query(pagination): Query<Pagination>
) -> impl IntoResponse {
    let user = DbUser::find_local(&data, &name).await?;
    if let Some(user) = user && let Some(local_user) = &user.local {
        let outbox = LocalUserOwnedCollection::new(
            local_user.following.clone(),
            0,
            pagination.page,
            10,
            async |_, _| Ok(vec![])
        ).await?;
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
