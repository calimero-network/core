use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use eyre::eyre;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{AccountId, BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use rand::{thread_rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{from_slice, json};
use sha2::{Digest, Sha256};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use tracing::{error, info};

use crate::verifysignature;
use futures_util::StreamExt;
use reqwest::Client;

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
    pub application_dir: camino::Utf8PathBuf,
}
pub(crate) struct ServiceState {
    application_dir: camino::Utf8PathBuf,
}

pub(crate) fn service(
    config: &crate::config::ServerConfig,
) -> eyre::Result<Option<(&'static str, Router)>> {
    let config = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin api is disabled");
            return Ok(None);
        }
    };
    let admin_path = "/admin-api";

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);
    let state = Arc::new(ServiceState {
        application_dir: config.application_dir.clone(),
    });
    let admin_router = Router::new()
        .route("/health", get(health_check_handler))
        .route("/root-key", post(create_root_key_handler))
        .route("/request-challenge", post(request_challenge_handler))
        .route(
            "/install-application",
            post(install_application_handler).layer(Extension(state)),
        )
        .layer(session_layer);

    Ok(Some((admin_path, admin_router)))
}

pub(crate) fn site(
    config: &crate::config::ServerConfig,
) -> eyre::Result<Option<(&'static str, ServeDir<SetStatus<ServeFile>>)>> {
    let _config = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin site is disabled");
            return Ok(None);
        }
    };
    let path = "/admin";

    let react_static_files_path = "./node-ui/dist";
    let react_app_serve_dir = ServeDir::new(react_static_files_path).not_found_service(
        ServeFile::new(format!("{}/index.html", react_static_files_path)),
    );

    Ok(Some((path, react_app_serve_dir)))
}

pub const CHALLENGE_KEY: &str = "challenge";

struct ApiResponse<T: Serialize> {
    payload: T,
}
impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        let body = serde_json::to_string(&self.payload).unwrap();
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    }
}

#[derive(Serialize)]
struct RequestChallengeBody {
    challenge: String,
}

pub async fn request_challenge_handler(session: Session) -> impl IntoResponse {
    if let Some(challenge) = session.get::<String>(CHALLENGE_KEY).await.ok().flatten() {
        ApiResponse {
            payload: RequestChallengeBody { challenge },
        }
        .into_response()
    } else {
        let challenge = generate_challenge();

        if let Err(err) = session.insert(CHALLENGE_KEY, &challenge).await {
            error!("Failed to insert challenge into session: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to insert challenge into session",
            )
                .into_response();
        }
        ApiResponse {
            payload: RequestChallengeBody { challenge },
        }
        .into_response()
    }
}

fn generate_random_bytes() -> [u8; 32] {
    let mut rng = thread_rng();
    let mut buf = [0u8; 32];
    rng.fill_bytes(&mut buf);
    buf
}

fn generate_challenge() -> String {
    let random_bytes = generate_random_bytes();
    let encoded = STANDARD.encode(&random_bytes);
    encoded
}

async fn health_check_handler() -> impl IntoResponse {
    (StatusCode::OK, "alive")
}

async fn create_root_key_handler(
    session: Session,
    Json(req): Json<PubKeyRequest>,
) -> impl IntoResponse {
    let message = "helloworld";
    let app = "me";

    match session.get::<String>(CHALLENGE_KEY).await.ok().flatten() {
        Some(challenge) => {
            if verifysignature::verify_signature(
                &challenge,
                message,
                app,
                &req.callback_url,
                &req.signature,
                &req.public_key,
            ) {
                (StatusCode::OK, "Root key created")
            } else {
                (StatusCode::BAD_REQUEST, "Invalid signature")
            }
        }
        _ => (StatusCode::BAD_REQUEST, "Challenge not found"),
    }
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub path: String,
    pub hash: String,
}

pub async fn get_release(application: &String, version: &String) -> eyre::Result<Release> {
    let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
    let request = methods::query::RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::CallFunction {
            account_id: "calimero-package-manager.testnet".parse()?,
            method_name: "get_release".to_string(),
            args: FunctionArgs::from(
                json!({
                    "name": application,
                    "version": version
                })
                .to_string()
                .into_bytes(),
            ),
        },
    };

    let response = client.call(request).await?;
    if let QueryResponseKind::CallResult(result) = response.kind {
        return Ok(from_slice::<Release>(&result.result)?);
    } else {
        Err(eyre!("Failed to fetch data from the rpc endpoint"))
    }
}

async fn abcd() -> eyre::Result<()> {
    let url = "http://example.com/bigfile.bin";
    let expected_hash = "your_expected_sha256_hash_here";

    let client = Client::new();
    let mut response = client.get(url).send().await?;

    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await? {
        hasher.update(&chunk);
        //chunk write to file
        //if verify not valid then delete file
    }

    let result = hasher.finalize();
    let result_str = format!("{:x}", result);

    if result_str == expected_hash {
        println!("Hash matches!");
    } else {
        println!("Hash does not match!");
    }

    Ok(())
}

pub async fn download_release(
    application: &String,
    release: &Release,
    dir: &camino::Utf8PathBuf,
) -> eyre::Result<()> {
    let base_path = format!("./{}/{}/{}", dir, application, &release.version);
    fs::create_dir_all(&base_path)?;

    let file_path = format!("{}/binary.wasm", base_path);
    let mut file = File::create(&file_path)?;

    let client = Client::new();
    let mut response = client.get(&release.path).send().await?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk)?;
    }
    let result = hasher.finalize();
    let hash = format!("{:x}", result);

    verify_release(&hash, &release.hash).await?;

    Ok(())
}

pub async fn verify_release(hash: &String, release_hash: &String) -> eyre::Result<()> {
    if hash != release_hash {
        println!("Hash does not match!");
        return Err(eyre!(
            "Release hash does not match the hash of the downloaded file"
        ));
    }
    Ok(())
}

pub async fn install_application(
    application: &String,
    version: &String,
    dir: &camino::Utf8PathBuf,
) -> eyre::Result<()> {
    let release = get_release(application, version).await?;
    download_release(application, &release, dir).await
}

async fn install_application_handler(
    Extension(state): Extension<Arc<ServiceState>>,
    session: Session,
    Json(req): Json<InstallApplicationRequest>,
) {
    match install_application(&req.application, &req.version, &state.application_dir).await {
        Ok(()) => (StatusCode::OK, "Application Installed"),
        Err(_) => (StatusCode::BAD_REQUEST, "Failed to install application"),
    };
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PubKeyRequest {
    // unused ATM, uncomment when used
    // account_id: String,
    public_key: String,
    signature: String,
    callback_url: String,
}

#[derive(Deserialize)]
struct InstallApplicationRequest {
    application: String,
    version: String,
}
