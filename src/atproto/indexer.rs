use std::{env, sync::Arc};

use anyhow::anyhow;
use atproto_client::{client::{AppPasswordAuth, Auth}, com::atproto::{repo::{CreateRecordRequest, CreateRecordResponse, GetRecordResponse, PutRecordRequest, PutRecordResponse, create_record, get_record, put_record}, server::{AppPasswordSession, create_session, refresh_session}}, errors::SimpleError};
use chrono::Utc;
use futures::lock::Mutex;
use rand::Rng;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, ETAG};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::sync::OnceCell;

use crate::{ActivityPubIcon, AppState, AtProtoAppState, Error, atproto::lexicon::{self, Blob}, reqwest_unsafe_client, types::utils::StableValue, with_slash};

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
    for _ in 1..13 {
        chars.push(ALPHABET[(tid & 0x1F) as usize]);
        tid >>= 5;
    }
    chars.reverse();
    chars.iter().collect()
}

pub(crate) async fn update_profile(state: &Arc<AppState>, atproto_state: &AtProtoAppState) -> Result<String, Error> {
    let auth = session(atproto_state).await?;

    let publication_rkey = &atproto_state.config.publication_rkey;

    let icon_blob = update_profile_blob(state, atproto_state, &auth).await?;

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
                _ => {}
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
                _ => {}
            }
        }
        GetRecordResponse::Error(simple) => {
            Err(anyhow!(simple.error_message()))?;
        }
    }

    Ok(publication_rkey.to_owned())
}

static LAST_ETAG: OnceCell<Arc<Mutex<Option<String>>>> = OnceCell::const_new();

pub(crate) async fn update_profile_blob(data: &Arc<AppState>, atproto_state: &AtProtoAppState, auth: &Auth) -> Result<Option<lexicon::Blob>, Error> {
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
            let existing: Option<(String, u32)> = sqlx::query("SELECT cid, size FROM atproto_blobs WHERE content_hash = ? AND media_type = ?")
                .bind(hash)
                .bind(&media_type)
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

                sqlx::query("INSERT INTO atproto_blobs (content_hash. media_type, cid, size) VALUES (?, ?, ?, ?) ON CONFLICT (content_hash, media_type) DO UPDATE SET
                    cid = excluded.cid,
                    size = excluded.size")
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
