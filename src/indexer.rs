use aws_lc_rs::digest::{SHA512, digest};
use axum::{body::Body, extract::Path, http::HeaderMap, response::{IntoResponse, Redirect, Response}};
use chrono::Utc;
use log::{info};
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value};
use sqlx::Row;
use url::Url;

use crate::{ActivityJson, AppState, JsonLike, db_conn, get_self_alias, log_and_fail, with_slash};

#[derive(Deserialize, Debug)]
struct Post {
    id: String,
    url: Url
}

#[derive(Deserialize, Debug)]
struct Posts {
    objects: Vec<Post>
}

#[derive(Serialize, Debug)]
pub struct ActPubActivity {
    #[serde(rename = "type")]
    pub activity_type: & 'static str,
    pub id: String,
    pub actor: Url,
    pub published: String,
    pub object: Value
}

pub async fn get_post(Path(path): Path<String>, headers: HeaderMap) -> Result<Response, StatusCode> {
    let conn = db_conn().await;
    if let Some(accept) = headers.get(header::ACCEPT) {
        if let Ok(accept_str) = accept.to_str() {
            if accept_str.contains("text/html") && !accept_str.contains("application/activity+json") && !accept_str.contains("application/ld+json") {
                let query: Option<String> = sqlx::query_scalar("SELECT human_url FROM posts WHERE id = ?")
                    .bind(path.as_str())
                    .fetch_optional(&conn)
                    .await.map_err(log_and_fail)?;
                return match query {
                    None => Err(StatusCode::NOT_FOUND),
                    Some(url) => {
                        Ok(Redirect::to(&url).into_response())
                    }
                }
            }
        }
    }
    let query: Option<String> = sqlx::query_scalar("SELECT static_url FROM posts WHERE id = ?")
        .bind(path.as_str())
        .fetch_optional(&conn)
        .await.map_err(log_and_fail)?;
    match query {
        None => Err(StatusCode::NOT_FOUND),
        Some(url) => {
            let stream = reqwest::get(url)
                .await.map_err(log_and_fail)?
                .error_for_status().map_err(log_and_fail)?
                .bytes_stream();
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, ActivityJson::<Value>::content_type())
                .body(Body::from_stream(stream))
                .map_err(log_and_fail)?
            )
        }
    }
}

pub async fn run_index(state: &AppState<'_>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    //TODO: Need to implement update checking for actor -- name, images, other configured stuff, their referenced URLs, and keys
    // Which means storing keys in the database (gotta do that, lol)

    let self_alias = get_self_alias(state)?;
    let self_alias_with_slash: Url = with_slash(&self_alias)?;
    let posts: Posts = reqwest::get(state.config.posts.as_str())
        .await?
        .error_for_status()?
        .json().await?;
    let conn = db_conn().await;
    for post in posts.objects {
        let post_id = self_alias_with_slash.join("objects/")?.join(&post.id)?;
        let content: Value = reqwest::get(post.url.as_str())
            .await?
            .error_for_status()?
            .json().await?;
        let canonical_content = serde_json::to_string(&content)?;
        let hash_digest = digest(&SHA512, &canonical_content.into_bytes());
        let hash = hash_digest.as_ref();
        let query = sqlx::query("SELECT valuehash FROM posts WHERE id = ?")
            .bind(post.id.as_str())
            .fetch_optional(&conn)
            .await?;
        let human_url = if let Value::Object(m) = &content {
            if let Some(Value::String(s)) = m.get("url") {
                Some(s)
            } else {
                None
            }
        } else {
            None
        };
        match query {
            None => {
                sqlx::query("INSERT INTO posts (id, valuehash, static_url, human_url) VALUES (?, ?, ?, ?)")
                    .bind(post.id.as_str())
                    .bind(hash)
                    .bind(post.url.as_str())
                    .bind(human_url)
                    .execute(&conn)
                    .await?;
                let now = Utc::now();
                let activity = ActPubActivity {
                    activity_type: "Create",
                    id: post_id.to_string(),
                    actor: self_alias.to_owned(),
                    published: now.to_rfc3339(),
                    object: content
                };
                sqlx::query("INSERT INTO outbox (published, value) VALUES (?, ?)")
                    .bind(now.timestamp_millis())
                    .bind(serde_json::to_string(&activity)?)
                    .execute(&conn)
                    .await?;
                info!("Indexed new post {} at {}", post_id, post.url);
            },
            Some(row) => {
                let old_hash: &[u8] = row.get("valuehash");
                if old_hash != hash {
                    sqlx::query("UPDATE posts SET valuehash = ?, static_url = ?, human_url = ? WHERE id = ?")
                        .bind(hash)
                        .bind(post.url.as_str())
                        .bind(human_url)
                        .bind(post.id.as_str())
                        .execute(&conn)
                        .await?;
                    let now = Utc::now();
                    let activity = ActPubActivity {
                        activity_type: "Update",
                        id: post_id.to_string(),
                        actor: self_alias.to_owned(),
                        published: now.to_rfc3339(),
                        object: content
                    };
                    sqlx::query("INSERT INTO outbox (published, value) VALUES (?, ?)")
                        .bind(now.timestamp_millis())
                        .bind(serde_json::to_string(&activity)?)
                        .execute(&conn)
                        .await?;
                    info!("Indexed updates to post {} from {}", post_id, post.url);
                }
            }
        }
    }
    Ok(())
}