use std::{fmt::Debug, fs::File, io::Read, path::Path, str::FromStr, sync::Arc, time::Duration};

use aws_lc_rs::{digest::{SHA512, digest}, encoding::AsDer, rsa, signature::KeyPair};
use axum::{Router, body::Body, extract::{Query, State}, http::{Method, StatusCode}, response::{IntoResponse, Response}, routing::{get, post}};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use clap::Parser;
use log::{error, info};
use reqwest::header;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Pool, Row, Sqlite, sqlite::{SqliteConnectOptions, SqlitePoolOptions}};
use tokio::{sync::OnceCell, time::interval};
use tower_http::cors::{AllowOrigin, CorsLayer};
use url::{Host, Url};

macro_rules! impl_into_response {
    ($name:ident) => {
        const _: () = {
            #[allow(unused)]
            fn assert_json_like<T: Serialize, U: JsonLike<T>>() {}

            // RFC 2056
            #[allow(unused)]
            fn assert_all<T: Serialize>() {
                assert_json_like::<T, $name<T>>();
            }
        };

        impl <T: Serialize> IntoResponse for $name<T> {
            fn into_response(self) -> Response {
                match serde_json::to_string(&self.held_value()) {
                    Ok(json) => Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, self.content_type())
                        .body(Body::from(json))
                        .unwrap(),
                    Err(_) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Serialization failed",
                    ).into_response(),
                }
            }
        }
    };
}

mod indexer;
mod inbox;

struct ActivityJson<T>(T);
impl_into_response!(ActivityJson);
impl <T: Serialize> JsonLike<T> for ActivityJson<T> {
    fn held_value(&self) -> &T { &self.0 }
    fn content_type(&self) -> &str { "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"" }
}

struct JsonRD<T>(T);
impl_into_response!(JsonRD);
impl <T: Serialize> JsonLike<T> for JsonRD<T> {
    fn held_value(&self) -> &T { &self.0 }
    fn content_type(&self) -> &str { "application/jrd+json" }
}

trait JsonLike<T: Serialize>: IntoResponse {
    fn held_value(&self) -> &T;
    fn content_type(&self) -> &str;
}

#[derive(Serialize, Debug)]
struct WebfingerLink {
    rel: String,
    #[serde(rename = "type")]
    link_type: String,
    href: String
}

#[derive(Serialize, Debug)]
struct WebfingerResponse {
    subject: String,
    aliases: Vec<String>,
    links: Vec<WebfingerLink>
}

#[derive(Deserialize, Debug)]
struct WebfingerParams {
    resource: String
}

async fn webfinger_handler(State(state): State<Arc<AppState<'_>>>, Query(params): Query<WebfingerParams>) -> Result<JsonRD<WebfingerResponse>, StatusCode> {
    if params.resource == format!("acct:{}@{}", state.config.username, state.config.host.host.to_string()) {
        let self_alias = get_self_alias(&state).map_err(log_and_fail)?;
        return Ok(JsonRD(WebfingerResponse {
            subject: params.resource,
            aliases: vec![
                self_alias.to_string(),
                state.config.homepage.to_string()
            ],
            links: vec![
                WebfingerLink {
                    rel: "self".to_owned(),
                    link_type: "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"".to_owned(),
                    href: self_alias.to_string()
                },
                WebfingerLink {
                    rel: "http://webfinger.net/rel/profile-page".to_owned(),
                    link_type: "text/html".to_owned(),
                    href: state.config.homepage.to_string()
                }
            ]
        }))
    }
    Err(StatusCode::NOT_FOUND)
}

fn default_context() -> Value {
    json!([
        "https://www.w3.org/ns/activitystreams",
        "https://w3id.org/security/v1",
        {
            "toot": "http://joinmastodon.org/ns#",
            "manuallyApprovesFollowers": "as:manuallyApprovesFollowers",
            "discoverable": "toot:discoverable",
            "memorial": "toot:memorial"
        }
    ])
}

#[derive(Serialize, Debug)]
struct ActPubPublicKey {
    id: String,
    owner: String,
    #[serde(rename = "publicKeyPem")]
    public_key_pem: String
}

#[derive(Serialize, Debug)]
struct ActPubActor {
    #[serde(rename = "@context")]
    context: Value,
    id: String,
    #[serde(rename = "type")]
    actor_type: & 'static str,
    webfinger: String,
    #[serde(rename = "preferredUsername")]
    preferred_username: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    url: Url,
    discoverable: bool,
    memorial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Value>,
    #[serde(rename = "publicKey")]
    public_key: ActPubPublicKey,
    #[serde(rename = "manuallyApprovesFollowers")]
    manually_approves_followers: bool,
    outbox: Url,
    inbox: Url
}

fn get_self_alias(state: &AppState<'_>) -> Result<Url, url::ParseError> {
    state.config.host.url.join(&format!("@{}", state.config.username))
}

fn get_actor(state: &AppState<'_>) -> Result<ActPubActor, Box<dyn std::error::Error + Send + Sync>> {
    let self_alias = get_self_alias(&state)?;
    
    let mut self_alias_with_slash = self_alias.clone();
    self_alias_with_slash.path_segments_mut().map_err(|_| {
        "Could not modify URL"
    })?.pop_if_empty().push("");

    Ok(ActPubActor {
        context: default_context(),
        id: self_alias.to_string(),
        webfinger: format!("{}@{}", state.config.username, state.config.host.host.to_string()),
        actor_type: "Person",
        preferred_username: state.config.username.clone(),
        name: state.config.name.clone(),
        summary: None,
        url: state.config.homepage.clone(),
        discoverable: true,
        memorial: false,
        icon: state.config.icon.clone(),
        image: state.config.image.clone(),
        public_key: ActPubPublicKey {
            id: format!("{}#main-key", self_alias.to_string()),
            owner: self_alias.to_string(),
            public_key_pem: format!(
                "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
                STANDARD.encode(state.keypair.public_key().as_der()?.as_ref())
            )
        },
        manually_approves_followers: false,
        outbox: self_alias_with_slash.join("outbox")?,
        inbox: self_alias_with_slash.join("inbox")?
    })
}

async fn actor_handler(State(state): State<Arc<AppState<'_>>>) -> Result<ActivityJson<ActPubActor>, StatusCode> {
    let actor: Result<ActPubActor, Box<dyn std::error::Error + Send + Sync>> = get_actor(&state);
    Ok(ActivityJson(actor.map_err(|e| {
        let e_d: Box<dyn std::error::Error> = e;
        e_d
    }).map_err(log_and_fail)?))
}

#[derive(Serialize, Debug)]
struct OrderedCollection {
    #[serde(rename = "@context")]
    context: Value,
    id: Url,
    #[serde(rename = "type")]
    collection_type: & 'static str,
    #[serde(rename = "totalItems")]
    total_items: u64,
    first: Url,
    last: Url
}

#[derive(Serialize, Debug)]
struct OrderedCollectionPage {
    #[serde(rename = "@context")]
    context: Value,
    id: Url,
    #[serde(rename = "type")]
    collection_type: & 'static str,
    #[serde(rename = "orderedItems")]
    items: Vec<Value>,
    #[serde(rename = "partOf")]
    part_of: Url,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev: Option<Url>
}

#[derive(Deserialize, Debug)]
struct OrderedCollectionParams {
    page: Option<u32>
}

async fn outbox_handler(State(state): State<Arc<AppState<'_>>>, Query(params): Query<OrderedCollectionParams>) -> Result<ActivityJson<Value>, StatusCode> {
    let self_alias = get_self_alias(&state).map_err(log_and_fail)?;
    
    let mut self_alias_with_slash = self_alias.clone();
    self_alias_with_slash.path_segments_mut().map_err(|_| {
        error!("Could not modify URL");
        StatusCode::INTERNAL_SERVER_ERROR
    })?.pop_if_empty().push("");
    let outbox_url = self_alias_with_slash.join("outbox").map_err(log_and_fail)?;

    let conn = db_conn().await;

    let total_count = sqlx::query_scalar("SELECT COUNT(*) FROM outbox")
        .fetch_one(&conn).await.map_err(log_and_fail)?;

    const PAGE_SIZE: u32 = 10;

    match params.page {
        None => {
            let mut first_page = outbox_url.clone();
            let mut last_page = outbox_url.clone();

            let pages = if total_count == 0 {
                1
            } else {
                ((total_count - 1) / (PAGE_SIZE as u64)) + 1
            };

            first_page.set_query(Some(&format!("page=1")));
            last_page.set_query(Some(&format!("page={}", pages)));

            Ok(ActivityJson(serde_json::to_value(OrderedCollection {
                context: default_context(),
                id: outbox_url,
                collection_type: "OrderedCollection",
                total_items: total_count,
                first: first_page,
                last: last_page
            }).map_err(log_and_fail)?))
        }
        Some(page_num) => {
            let outbox_rows: Result<Vec<_>, _> = sqlx::query("SELECT value FROM outbox ORDER BY published DESC, id DESC LIMIT ? OFFSET ?")
                .bind(PAGE_SIZE)
                .bind(PAGE_SIZE * (page_num - 1))
                .fetch_all(&conn)
                .await.map_err(log_and_fail)?
                .iter().map(|row| {
                    let s = row.get::<String, &str>("value");
                    serde_json::from_str::<Value>(&s).map_err(log_and_fail)
                })
                .collect();
            let outbox_rows = outbox_rows?;

            let mut this_page = outbox_url.clone();
            let mut next_page = outbox_url.clone();
            let mut prev_page = outbox_url.clone();

            this_page.set_query(Some(&format!("page={}", page_num)));
            next_page.set_query(Some(&format!("page={}", page_num+1)));
            prev_page.set_query(Some(&format!("page={}", page_num-1)));

            Ok(ActivityJson(serde_json::to_value(OrderedCollectionPage {
                context: default_context(),
                id: this_page,
                collection_type: "OrderedCollectionPage",
                items: outbox_rows,
                part_of: outbox_url,
                next: if (PAGE_SIZE as u64) * (page_num as u64) >= total_count {
                    None
                } else {
                    Some(next_page)
                },
                prev: if page_num == 1 {
                    None
                } else {
                    Some(prev_page)
                }
            }).map_err(log_and_fail)?))
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(try_from = "Url")]
struct HostUrl {
    host: Host<String>,
    url: Url
}

impl TryFrom<Url> for HostUrl {
    type Error = &'static str;

    fn try_from(mut value: Url) -> Result<Self, Self::Error> {
        if value.path() != "/" {
            Err("Must be at root of origin")
        } else {
            match value.host() {
                None => Err("No host present"),
                Some(host) => {
                    let host = host.to_owned();
                    value.set_query(None);
                    value.set_path("/");
                    Ok(HostUrl {
                        host: host.to_owned(),
                        url: value
                    })
                }
            }
        }
    }
}

#[derive(Deserialize, Debug)]
struct Config {
    port: u16,
    username: String,
    host: HostUrl,
    homepage: Url,
    icon: Option<Value>,
    image: Option<Value>,
    name: String,
    posts: Url
}

#[derive(Parser)]
struct Cli {
    config: std::path::PathBuf
}

struct AppState<'a> {
    config: & 'a Config,
    keypair: rsa::KeyPair
}

fn read_config_from_file<P: AsRef<Path>>(path: P) -> Result<Config, tokio::io::Error> {
    let mut file = File::open(path)?;
    
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    
    let config: Config = serde_json::from_str(&contents)?;
    
    Ok(config)
}

fn log_and_fail<E: Into<Box<dyn std::error::Error>>>(e: E) -> StatusCode {
    let e_d: Box<dyn Debug> = e.into();
    error!("{e_d:?}");
    StatusCode::INTERNAL_SERVER_ERROR
}

trait Loggable<R> {
    fn log_or(self, r: R) -> R;

    fn log(self) -> ()
    where
        Self: Loggable<()> + Sized
    {
        self.log_or(())
    }
}

impl <R, E: Debug> Loggable<R> for Result<R, E> {
    fn log_or(self: Self, value: R) -> R {
        match self {
            Ok(r) => r,
            Err(e) => {
                error!("{e:?}");
                value
            },
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let args = Cli::parse();

    let conn = db_conn().await;
    sqlx::migrate!()
        .run(&conn)
        .await?;

    let config = Box::leak(Box::new(read_config_from_file(args.config)?));

    let maybe_key = sqlx::query("SELECT value FROM keys")
        .fetch_optional(&conn)
        .await?;
    let private_key = match maybe_key {
        Some(row) => {
            let s: &[u8] = row.get("value");
            rsa::KeyPair::from_pkcs8(s)?
        },
        None => {
            let key = rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa4096)?;
            sqlx::query("INSERT INTO keys (value) VALUES (?)")
                .bind(key.as_der()?.as_ref())
                .execute(&conn)
                .await?;
            key
        }
    };

    let state = Arc::new(AppState {
        config: config,
        keypair: private_key
    });

    let actor_value = serde_json::to_value(get_actor(&state)?)?;
    let actor_content = serde_json::to_string(&actor_value)?;
    let hash_digest = digest(&SHA512, &actor_content.into_bytes());
    let hash = hash_digest.as_ref();
    let has_changed = match sqlx::query("SELECT (hash) FROM actorstate")
        .fetch_optional(&conn).await? {
            None => true,
            Some(old_hash) => {
                if old_hash.get::<&[u8], &str>("hash") == hash {
                    false
                } else {
                    sqlx::query("DELETE FROM actorstate").execute(&conn).await?;
                    true
                }
            }
        };
    if has_changed {
        let self_alias = get_self_alias(&state)?;
        sqlx::query("INSERT INTO actorstate (hash) VALUES (?)")
            .bind(hash)
            .execute(&conn).await?;
        let now = Utc::now();
        let activity = indexer::ActPubActivity {
            activity_type: "Update",
            id: self_alias.to_string(),
            actor: self_alias.to_owned(),
            published: now.to_rfc3339(),
            object: actor_value
        };
        sqlx::query("INSERT INTO outbox (published, value) VALUES (?, ?)")
            .bind(now.timestamp_millis())
            .bind(serde_json::to_string(&activity)?)
            .execute(&conn)
            .await?;
        info!("Indexed updates to actor state");
    }

    let state_clone = state.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(60));
        
        let state = state_clone;

        loop {
            // Wait for the next tick (the first tick completes immediately)
            ticker.tick().await;
            
            indexer::run_index(&state).await.log();
        }
    });

    let cors_layer = CorsLayer::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods([Method::GET]);

    let app = Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route(&format!("/@{}", state.config.username), get(actor_handler))
        .route(&format!("/@{}/outbox", state.config.username), get(outbox_handler))
        .route(&format!("/@{}/inbox", state.config.username), post(inbox::inbox_post))
        .layer(cors_layer)
        .with_state(state);
 
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port)).await.unwrap();
    info!("Listening on {}", config.host.url.as_str());
    axum::serve(listener, app).await?;
    Ok(())
}

static DB_CONN: OnceCell<Pool<Sqlite>> = OnceCell::const_new();

async fn db_conn() -> Pool<Sqlite> {
    let db_url = "sqlite://main.db";

    DB_CONN
        .get_or_init(|| async {
            SqlitePoolOptions::new()
                .max_connections(20)
                .idle_timeout(Duration::from_secs(60))
                .acquire_timeout(Duration::from_secs(5))
                .connect_with(
                    SqliteConnectOptions::from_str(db_url)
                        .unwrap()
                        .create_if_missing(true)
                        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete),
                )
                .await
                .unwrap()
        })
        .await
        .clone()
}