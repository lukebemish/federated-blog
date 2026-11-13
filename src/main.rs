use std::{fmt::{Debug, Display, Formatter}, fs::File, io::Read, path::Path, str::FromStr, sync::Arc, time::Duration};

use activitypub_federation::config::{FederationConfig, FederationMiddleware};
use axum::{Router, http::{Method, StatusCode}, routing::{get, post}};
use clap::Parser;
use log::{LevelFilter, error, info};
use reqwest_ssrf_guard::Acl;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Pool, Sqlite, sqlite::{SqliteConnectOptions, SqlitePoolOptions}};
use tokio::{sync::OnceCell, time::interval};
use tower_http::cors::{AllowOrigin, CorsLayer};
use url::{Host, Url};

use crate::types::objects::person::DbUser;

mod types;
mod indexer;
mod http;

fn default_context() -> Value {
    json!([
        "https://www.w3.org/ns/activitystreams",
        "https://w3id.org/security/v1",
        {
            "toot": "http://joinmastodon.org/ns#",
            "manuallyApprovesFollowers": "as:manuallyApprovesFollowers",
            "discoverable": "toot:discoverable",
            "indexable": "toot:indexable",
            "memorial": "toot:memorial",
            "Hashtag": "as:Hashtag",
            "schema": "http://schema.org#",
            "PropertyValue": "schema:PropertyValue",
            "value": "schema:value"
        }
    ])
}

fn get_self_alias(state: &AppState) -> Result<Url, url::ParseError> {
    state.config.host.url.join(&format!("@{}", state.config.username))
}

#[derive(Debug)]
struct UrlError {
    message: String
}
impl Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UrlError({})", self.message)
    }
}
impl std::error::Error for UrlError {}


fn with_slash(url: &Url) -> Result<Url, UrlError> {
    let mut with_slash = url.clone();
    with_slash.path_segments_mut().map_err(|_| {
        UrlError { message: "Could not modify URL".to_owned() }
    })?.pop_if_empty().push("");
    Ok(with_slash)
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
    #[serde(default)]
    attribution_domains: Vec<String>,
    icon: Option<Value>,
    image: Option<Value>,
    name: String,
    posts: Url
}

#[derive(Parser)]
struct Cli {
    config: std::path::PathBuf,
    #[arg(long)]
    dev: bool
}

pub(crate) struct AppState {
    config: Config,
    database: Pool<Sqlite>
}

fn read_config_from_file<P: AsRef<Path>>(path: P) -> Result<Config, tokio::io::Error> {
    let mut file = File::open(path)?;
    
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    
    let config: Config = serde_json::from_str(&contents)?;
    
    Ok(config)
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
async fn main() -> Result<(), Error> {
    env_logger::Builder::from_default_env()
        .filter_level(LevelFilter::Warn)
        .filter_module("activitypub_federation", LevelFilter::Info)
        .filter_module("federated_blog", LevelFilter::Info)
        .format_timestamp(None)
        .init();

    let args = args().await;

    let conn = db_conn().await;
    sqlx::migrate!()
        .run(&conn)
        .await?;

    let config = read_config_from_file(&args.config)?;

    let state = Arc::new(AppState {
        config: config,
        database: conn
    });

    let ap_config = FederationConfig::builder()
        .domain(&state.config.host.host.to_string())
        .app_data(state.clone())
        .build()
        .await?;

    let local_user = DbUser::new_local(&ap_config.to_request_data()).await?;

    // Now rebuild it with a signing actor

    let ap_config = FederationConfig::builder()
        .domain(&state.config.host.host.to_string())
        .app_data(state.clone())
        .http_signature_compat(true) // For Mastodon
        .signed_fetch_actor(&local_user)
        .build()
        .await?;

    let data = ap_config.to_request_data();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(60));
        
        loop {
            // Wait for the next tick (the first tick completes immediately)
            ticker.tick().await;
            
            indexer::run_index(&data).await.log();
        }
    });

    let cors_layer = CorsLayer::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods([Method::GET]);

    let app = Router::new()
        .route("/@{user}/objects/{*path}", get(http::http_get_article))
        .route("/@{user}", get(http::http_get_user))
        .route("/@{user}/inbox", post(http::http_post_user_inbox))
        .route("/@{user}/outbox", get(http::http_get_user_outbox))
        .route("/@{user}/followers", get(http::http_get_user_followers))
        .route("/@{user}/following", get(http::http_get_user_following))
        .route("/activities/{id}", get(http::http_get_outbound_activity))
        .route("/.well-known/webfinger", get(http::webfinger))
        .layer(FederationMiddleware::new(ap_config))
        .layer(cors_layer)
        .with_state(state.clone());
 
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", state.config.port)).await.unwrap();
    info!("Listening on {}", state.config.host.url.as_str());
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug)]
pub(crate) enum Error {
    Simple(anyhow::Error),
    Status(StatusCode)
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self {
            Error::Simple(e) => std::fmt::Display::fmt(e, f),
            Error::Status(s) => std::fmt::Display::fmt(s, f)
        }
    }
}

impl<T> From<T> for Error
where
    T: Into<anyhow::Error>,
{
    fn from(t: T) -> Self {
        Error::Simple(t.into())
    }
}

static ARGS: OnceCell<Arc<Cli>> = OnceCell::const_new();

async fn args() -> Arc<Cli> {
    ARGS.get_or_init(|| async {
        Arc::new(Cli::parse())
    }).await.clone()
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
                        .foreign_keys(true) 
                        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete),
                )
                .await
                .unwrap()
        })
        .await
        .clone()
}

static CLIENT: tokio::sync::OnceCell<reqwest_middleware::ClientWithMiddleware> = OnceCell::const_new();

async fn reqwest_client() -> reqwest_middleware::ClientWithMiddleware {
    CLIENT.get_or_init(|| async {
        if args().await.dev {
            reqwest_middleware::ClientBuilder::new(
                reqwest::Client::builder()
                    .build().unwrap()
            ).build()
        } else {
            let acl = Acl::new()
                .deny_local_network()
                .deny_non_default_ports();

            let inner = acl
                .configure(reqwest::Client::builder())
                .no_proxy()
                .https_only(true)
                .build().unwrap();

            acl
                .configure_middleware(reqwest_middleware::ClientBuilder::new(inner))              // validate_url on every request
                .build()
        }
    }).await.clone()
}