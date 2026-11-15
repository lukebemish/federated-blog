use std::{env, sync::Arc};

use anyhow::anyhow;
use atproto_client::{client::{AppPasswordAuth, Auth}, com::atproto::{repo::{CreateRecordRequest, CreateRecordResponse, GetRecordResponse, PutRecordRequest, PutRecordResponse, create_record, get_blob, get_record, put_record}, server::{AppPasswordSession, create_session, refresh_session}}, errors::SimpleError};
use chrono::{DateTime, Utc};
use futures::lock::Mutex;
use rand::Rng;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, ETAG};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::sync::OnceCell;
use xxhash_rust::xxh32::xxh32;

use crate::{ActivityPubIcon, AppState, AtProtoAppState, Error, atproto::lexicon::{self, Blob, SiteStandardDocument, SiteStandardDocumentType, SiteStandardPublication}, html, reqwest_unsafe_client, types::{objects::article::DbLocalArticle, utils::StableValue}, with_slash};

static SESSION_AUTH: OnceCell<Arc<Mutex<AppPasswordSession>>> = OnceCell::const_new();

pub(crate) async fn session(atproto_state: &AtProtoAppState) -> Result<Auth, Error> {
    let session_holder = SESSION_AUTH.get_or_try_init(|| async {
        let client = reqwest_unsafe_client().await;

        Ok::<Arc<Mutex<AppPasswordSession>>, anyhow::Error>(Arc::new(Mutex::new(create_session(
            &client,
            &atproto_state.config.base_url.as_str(),
            &atproto_state.config.did,
            &atproto_state.password,
            None
        ).await?)))
    }).await?;

    let mut held = session_holder.lock().await;
    
    let client = reqwest_unsafe_client().await;

    let response = refresh_session(
        &client,
        &atproto_state.config.base_url.as_str(),
        &held.refresh_jwt
    ).await?;

    held.did = response.did;
    held.handle = response.handle;
    held.access_jwt = response.access_jwt;
    held.refresh_jwt = response.refresh_jwt;

    Ok(Auth::AppPassword(AppPasswordAuth {
        access_token: held.access_jwt.clone()
    }))
}

pub(crate) fn atproto_password() -> Result<Option<String>, Error> {
    match env::var_os("ATPROTO_APP_PASSWORD") {
        None => Ok(None),
        Some(password) => {
            let password = password.into_string().map_err(|_| anyhow!("Not an error"))?;
            Ok(Some(password))
        }
    }
}

static ALPHABET: [char; 32] = ['2','3','4','5','6','7','a','b','c','d','e','f','g','h','i','j','k','l','m','n','o','p','q','r','s','t','u','v','w','x','y','z'];

pub(crate) fn generate_tid() -> String {
    let now = Utc::now();
    let micro = now.timestamp_micros();
    let clock_id = rand::rng().next_u32() as i64;
    let mut tid = (
        micro << 10
    ) | (
        clock_id & 0x3ff
    );
    tid = 0x1FFFFFFFFFFFFF & tid;
    let mut chars = vec![];
    for _ in 0..13 {
        chars.push(ALPHABET[(tid & 0x1F) as usize]);
        tid >>= 5;
    }
    chars.reverse();
    chars.iter().collect()
}

pub(crate) fn calculate_tid(date: &DateTime<Utc>, id_path: &String) -> String {
    let micro = date.timestamp_micros();
    let clock_id = xxh32(id_path.as_bytes(), 0) as i64;
    let mut tid = (
        micro << 10
    ) | (
        clock_id & 0x3FF
    );
    tid = 0x1FFFFFFFFFFFFF & tid;
    let mut chars = vec![];
    for _ in 0..13 {
        chars.push(ALPHABET[(tid & 0x1F) as usize]);
        tid >>= 5;
    }
    chars.reverse();
    chars.iter().collect()
}

pub(crate) async fn update_article(_state: &Arc<AppState>, atproto_state: &AtProtoAppState, auth: &Auth, id_path: &String, article: &DbLocalArticle, publication_rkey: &String) -> Result<(), Error> {
    let published = match article.extra.get("published").and_then(|s| {
        let o = match s {
            StableValue::String(s) => DateTime::parse_from_rfc3339(s).ok().map(|d| d.to_utc()),
            _ => None,
        };
        if o.is_none() {
            log::warn!("Article {} has a 'published' date but it does not seem to be a valid date-time: {:?}", id_path, s);
        }
        o
    }) {
        None => {
            log::warn!("Cannot federated article {} without valid published date to AT Protocol; impossible to calculate a stable TID rkey", id_path);
            return Ok(());
        },
        Some(d) => d
    };
    let content = match article.extra.get("content").and_then(|c| match c {
        StableValue::String(s) => Some(s),
        _ => None
    }) {
        None => {
            log::warn!("Cannot federated article {} without string content", id_path);
            return Ok(());
        },
        Some(c) => c
    };
    let summary = match article.extra.get("summary").and_then(|c| match c {
        StableValue::String(s) => Some(s),
        _ => None
    }) {
        None => None,
        Some(html) => Some(html::strip_tags(html)?)
    };
    let tid = calculate_tid(&published, id_path);
    let document = SiteStandardDocument {
        kind: SiteStandardDocumentType(),
        site: format!("at://{}/site.standard.publication/{}", atproto_state.config.did, publication_rkey),
        path: id_path.to_owned(),
        description: summary,
        cover_image: None,
        text_content: None,
        published_at: published,
        title: article.name.clone(),
        content: Some(json!({
            "$type": "org.wordpress.html",
            "html": content.clone()
        }).into())
    };

    let client = reqwest_unsafe_client().await;

    let record_result = get_record(
        &client,
        &auth,
        atproto_state.config.base_url.as_str(),
        &atproto_state.config.did,
        "site.standard.document",
        &tid,
        None
    ).await?;

    match record_result {
        GetRecordResponse::Record {
            value, ..
        } => {
            let old_value: StableValue = serde_json::from_value(value)?;
            let new_value: StableValue = serde_json::to_value(document)?.into();
            if old_value == new_value {
                return Ok(());
            }
            match put_record(
                &client,
                &auth,
                atproto_state.config.base_url.as_str(),
                PutRecordRequest {
                    repo: atproto_state.config.did.clone(),
                    collection: "site.standard.document".to_owned(),
                    record_key: tid.clone(),
                    record: new_value,
                    swap_commit: None,
                    swap_record: None,
                    validate: true
                }
            ).await? {
                PutRecordResponse::Error(simple) => {
                    Err(anyhow!(simple.error_message()))?;
                },
                PutRecordResponse::StrongRef { uri, .. } => {
                    log::info!("Updated site.standard.document {}", uri);
                }
            }
        },
        GetRecordResponse::Error(ref simple @ SimpleError { error: Some(ref error), .. })
            if let "InvalidRequest" | "RecordNotFound" = error.as_str() => {
                // Record does not exist, we will attempt to create it
                match create_record(
                    &client,
                    &auth,
                    atproto_state.config.base_url.as_str(),
                    CreateRecordRequest {
                        repo: atproto_state.config.did.clone(),
                        collection: "site.standard.document".to_owned(),
                        record_key: Some(tid.clone()),
                        record: document,
                        swap_commit: None,
                        validate: true
                    }
                ).await? {
                    CreateRecordResponse::Error(simple) => {
                        Err(anyhow!(simple.error_message()))?;
                    },
                    CreateRecordResponse::StrongRef { uri, .. } => {
                        log::info!("Created site.standard.document {}", uri);
                    }
                }
            }
        GetRecordResponse::Error(simple) => {
            Err(anyhow!(simple.error_message()))?;
        }
    }

    Ok(())
}

pub(crate) async fn update_profile(state: &Arc<AppState>, atproto_state: &AtProtoAppState, auth: &Auth) -> Result<String, Error> {
    let publication_rkey = &atproto_state.config.publication_rkey;

    let icon_blob = update_profile_blob(state, atproto_state, &auth, publication_rkey).await?;

    let mut publication = atproto_state.config.publication.clone();
    if let Some(blob) = icon_blob {
        publication.icon = Some(blob);
    }

    let client = reqwest_unsafe_client().await;

    let record_result = get_record(
        &client,
        &auth,
        atproto_state.config.base_url.as_str(),
        &atproto_state.config.did,
        "site.standard.publication",
        publication_rkey,
        None
    ).await?;

    match record_result {
        GetRecordResponse::Record {
            value, ..
        } => {
            let old_publication: SiteStandardPublication = serde_json::from_value(value.clone())?;
            if let Some(blob) = old_publication.icon && let None = publication.icon {
                publication.icon = Some(blob);
            }
            let old_value: StableValue = serde_json::from_value(value)?;
            let new_value: StableValue = serde_json::to_value(publication)?.into();
            if old_value == new_value {
                return Ok(publication_rkey.to_owned())
            }
            match put_record(
                &client,
                &auth,
                atproto_state.config.base_url.as_str(),
                PutRecordRequest {
                    repo: atproto_state.config.did.clone(),
                    collection: "site.standard.publication".to_owned(),
                    record_key: publication_rkey.clone(),
                    record: new_value,
                    swap_commit: None,
                    swap_record: None,
                    validate: true
                }
            ).await? {
                PutRecordResponse::Error(simple) => {
                    Err(anyhow!(simple.error_message()))?;
                },
                PutRecordResponse::StrongRef { uri, .. } => {
                    log::info!("Updated site.standard.publication {}", uri);
                }
            }
        },
        GetRecordResponse::Error(ref simple @ SimpleError { error: Some(ref error), .. })
            if let "InvalidRequest" | "RecordNotFound" = error.as_str() => {
                // Record does not exist, we will attempt to create it
                match create_record(
                    &client,
                    &auth,
                    atproto_state.config.base_url.as_str(),
                    CreateRecordRequest {
                        repo: atproto_state.config.did.clone(),
                        collection: "site.standard.publication".to_owned(),
                        record_key: Some(publication_rkey.clone()),
                        record: publication,
                        swap_commit: None,
                        validate: true
                    }
                ).await? {
                    CreateRecordResponse::Error(simple) => {
                        Err(anyhow!(simple.error_message()))?;
                    },
                    CreateRecordResponse::StrongRef { uri, .. } => {
                        log::info!("Created site.standard.publication {}", uri);
                    }
                }
            }
        GetRecordResponse::Error(simple) => {
            Err(anyhow!(simple.error_message()))?;
        }
    }

    Ok(publication_rkey.to_owned())
}

static LAST_ETAG: OnceCell<Arc<Mutex<Option<String>>>> = OnceCell::const_new();

pub(crate) async fn update_profile_blob(data: &Arc<AppState>, atproto_state: &AtProtoAppState, auth: &Auth, publication_rkey: &String) -> Result<Option<lexicon::Blob>, Error> {
    if let Some(ActivityPubIcon { url, .. }) = &data.config.icon {
        let etag_holder = LAST_ETAG.get_or_init(|| async {
            Arc::new(Mutex::new(None))
        }).await;

        let client = reqwest_unsafe_client().await;

        let response = client.head(url.clone())
            .send().await?.error_for_status()?;
        let etag = response.headers().get(ETAG);

        let should_recheck = match etag {
            None => true,
            Some(new_etag) => {
                let new_etag = new_etag.to_str()?;
                let mut holder = etag_holder.lock().await;
                match holder.replace(new_etag.to_owned()) {
                    None => true,
                    Some(old_etag) => {
                        if old_etag == new_etag {
                            true
                        } else {
                            false
                        }
                    }
                }
            }
        };

        if should_recheck {
            let response = client.get(url.clone())
                .send().await?.error_for_status()?;
            let media_type = response.headers().get(CONTENT_TYPE).map(|v| v.to_str())
                .unwrap_or_else(|| Ok(""))?.to_owned();
            let bytes = response.bytes().await?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);

            let hash: &[u8] = &hasher.finalize();
            let existing: Option<(String, u32)> = sqlx::query("SELECT cid, size FROM atproto_blobs WHERE content_hash = ? AND media_type = ? AND rkey = ?")
                .bind(hash)
                .bind(&media_type)
                .bind(publication_rkey)
                .fetch_optional(&data.database)
                .await.map(|row| {
                    row.map(|row| (row.get("cid"), row.get("size")))
                })?;

            if let Some(existing) = existing {
                return Ok(Some(Blob {
                    kind: Default::default(),
                    mime_type: media_type,
                    size: existing.1,
                    blob_ref: existing.0
                }));
            }

            if let Auth::AppPassword(password_auth) = auth {
                #[derive(Deserialize)]
                struct BlobObject {
                    blob: lexicon::Blob
                }
                let blob_object: BlobObject = client.post(
                    with_slash(&atproto_state.base_url)?.join("xrpc/com.atproto.repo.uploadBlob")?
                ).bearer_auth(&password_auth.access_token)
                    .header(CONTENT_TYPE, &media_type)
                    .header(CONTENT_LENGTH, bytes.len())
                    .body(bytes)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                let blob = blob_object.blob;

                sqlx::query("INSERT INTO atproto_blobs (rkey, content_hash, media_type, cid, size) VALUES (?, ?, ?, ?, ?) ON CONFLICT (rkey) DO UPDATE SET
                    content_hash = excluded.content_hash,
                    media_type = excluded.media_type,
                    cid = excluded.cid,
                    size = excluded.size")
                    .bind(publication_rkey)
                    .bind(hash)
                    .bind(&media_type)
                    .bind(&blob.blob_ref)
                    .bind(&blob.size)
                    .execute(&data.database)
                    .await?;

                return Ok(Some(blob));
            }
            
        }

        Ok(None)
    } else {
        Ok(None)
    }
}
