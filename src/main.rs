use std::{fs::File, io::Read, path::Path, str::FromStr, sync::Arc, time::Duration};

use aws_lc_rs::{encoding::AsDer, rsa, signature::KeyPair};
use axum::{Json, Router, extract::{Query, State}, http::{Method, StatusCode}, routing::get};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Encode, Pool, Sqlite, sqlite::{SqliteConnectOptions, SqlitePoolOptions}};
use tokio::sync::OnceCell;
use tower_http::cors::{AllowOrigin, CorsLayer};
use url::{Host, Url};

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
        let self_alias = state.config.host.url.join(&format!("@{}", state.config.username)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    public_key: ActPubPublicKey
}

async fn actor_handler(State(state): State<Arc<AppState<'_>>>) -> Result<Json<ActPubActor>, StatusCode> {
    let self_alias = state.config.host.url.join(&format!("@{}", state.config.username)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
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
                STANDARD.encode(state.keypair.public_key().as_der().unwrap().as_ref())
            )
        }
    }))
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
    name: String
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Cli::parse();

    let conn = db_conn().await;

    let config = Box::leak(Box::new(read_config_from_file(args.config)?));

    let state = Arc::new(AppState {
        config: config,
        keypair: rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa4096)?
    });

    let cors_layer = CorsLayer::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods([Method::GET]);

    let app = Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route(&format!("/@{}", state.config.username), get(actor_handler))
        .layer(cors_layer)
        .with_state(state);
 
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port)).await.unwrap();
    println!("Listening on http://localhost:3000");
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