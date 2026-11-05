use aws_lc_rs::digest::{SHA512, digest};
use chrono::Utc;
use log::{info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use url::Url;

use crate::{AppState, db_conn, get_self_alias};

#[derive(Deserialize, Debug)]
struct Posts {
    objects: Vec<Url>
}

#[derive(Serialize, Debug)]
struct ActPubActivity {
    #[serde(rename = "type")]
    activity_type: & 'static str,
    id: String,
    actor: Url,
    published: String,
    object: Value
}

pub async fn run_index(state: &AppState<'_>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let self_alias: Url = get_self_alias(state)?;
    let posts: Posts = reqwest::get(state.config.posts.as_str())
        .await?
        .error_for_status()?
        .json().await?;
    let conn = db_conn().await;
    for post_url in posts.objects {
        let content: Value = reqwest::get(post_url.as_str())
            .await?
            .error_for_status()?
            .json().await?;
        let canonical_content = serde_json::to_string(&content)?;
        let hash_digest = digest(&SHA512, &canonical_content.into_bytes());
        let hash = hash_digest.as_ref();
        let query = sqlx::query("SELECT valuehash FROM posts WHERE id = ?")
            .bind(post_url.as_str())
            .fetch_optional(&conn)
            .await?;
        match query {
            None => {
                sqlx::query("INSERT INTO posts (id, valuehash) VALUES (?, ?)")
                    .bind(post_url.as_str())
                    .bind(hash)
                    .execute(&conn)
                    .await?;
                let now = Utc::now();
                let activity = ActPubActivity {
                    activity_type: "Create",
                    id: post_url.to_string(),
                    actor: self_alias.to_owned(),
                    published: now.to_rfc3339(),
                    object: content
                };
                sqlx::query("INSERT INTO outbox (published, value) VALUES (?, ?)")
                    .bind(now.timestamp_millis())
                    .bind(serde_json::to_string(&activity)?)
                    .execute(&conn)
                    .await?;
                info!("Indexed new post {post_url}");
            },
            Some(row) => {
                let old_hash: &[u8] = row.get("valuehash");
                if old_hash != hash {
                    sqlx::query("UPDATE posts SET valuehash = ? WHERE id = ?")
                        .bind(hash)
                        .bind(post_url.as_str())
                        .execute(&conn)
                        .await?;
                    let now = Utc::now();
                    let activity = ActPubActivity {
                        activity_type: "Update",
                        id: post_url.to_string(),
                        actor: self_alias.to_owned(),
                        published: now.to_rfc3339(),
                        object: content
                    };
                    sqlx::query("INSERT INTO outbox (published, value) VALUES (?, ?)")
                        .bind(now.timestamp_millis())
                        .bind(serde_json::to_string(&activity)?)
                        .execute(&conn)
                        .await?;
                    info!("Indexed updates to post {post_url}");
                }
            }
        }
    }
    Ok(())
}