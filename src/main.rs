use std::{fmt::Debug, fs::File, io::Read, path::Path, str::FromStr, sync::Arc, time::Duration};

use aws_lc_rs::{encoding::AsDer, rsa, signature::KeyPair};
use axum::{Json, Router, extract::{Query, State}, http::{Method, StatusCode}, routing::get};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Parser;
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Pool, Row, Sqlite, sqlite::{SqliteConnectOptions, SqlitePoolOptions}};
use tokio::{sync::OnceCell, time::interval};
use tower_http::cors::{AllowOrigin, CorsLayer};
use url::{Host, Url};

mod indexer;

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

async fn webfinger_handler(State(state): State<Arc<AppState<'_>>>, Query(params): Query<WebfingerParams>) -> Result<Json<WebfingerResponse>, StatusCode> {
    if params.resource == format!("acct:{}@{}", state.config.username, state.config.host.host.to_string()) {
        let self_alias = get_self_alias(&state).map_err(log_and_fail)?;
        return Ok(Json(WebfingerResponse {
            subject: params.resource,
            aliases: vec![
                self_alias.to_string(),
                state.config.homepage.to_string()
            ],
            links: vec![
                WebfingerLink {
                    rel: "self".to_owned(),
                    link_type: "application/activity+json".to_owned(),
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
    context: & 'static str,
    id: String,
    #[serde(rename = "type")]
    actor_type: & 'static str,
    #[serde(rename = "preferredUsername")]
    preferred_username: String,
    name: String,
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
    outbox: Url
}

fn get_self_alias(state: &AppState<'_>) -> Result<Url, url::ParseError> {
    state.config.host.url.join(&format!("@{}", state.config.username))
}

async fn actor_handler(State(state): State<Arc<AppState<'_>>>) -> Result<Json<ActPubActor>, StatusCode> {
    let self_alias = get_self_alias(&state).map_err(log_and_fail)?;
    
    let mut self_alias_with_slash = self_alias.clone();
    self_alias_with_slash.path_segments_mut().map_err(|_| {
        error!("Could not modify URL");
        StatusCode::INTERNAL_SERVER_ERROR
    })?.pop_if_empty().push("");

    Ok(Json(ActPubActor {
        context: "https://www.w3.org/ns/activitystreams",
        id: self_alias.to_string(),
        actor_type: "Persion",
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
                STANDARD.encode(state.keypair.public_key().as_der().map_err(log_and_fail)?.as_ref())
            )
        },
        manually_approves_followers: false,
        outbox: self_alias_with_slash.join("outbox").map_err(log_and_fail)?
    }))
}

#[derive(Serialize, Debug)]
struct OrderedCollection {
    #[serde(rename = "@context")]
    context: & 'static str,
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
    context: & 'static str,
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

async fn outbox_handler(State(state): State<Arc<AppState<'_>>>, Query(params): Query<OrderedCollectionParams>) -> Result<Json<Value>, StatusCode> {
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

            Ok(Json(serde_json::to_value(OrderedCollection {
                context: "https://www.w3.org/ns/activitystreams",
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

            Ok(Json(serde_json::to_value(OrderedCollectionPage {
                context: "https://www.w3.org/ns/activitystreams",
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

fn log_and_fail(e: impl std::error::Error) -> StatusCode {
    error!("{e:?}");
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

    let state = Arc::new(AppState {
        config: config,
        keypair: rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa4096)?
    });

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