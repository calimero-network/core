use std::error::Error;
use std::fmt::{self, Display, Formatter};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use rand::{thread_rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{from_slice, json};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use tracing::{error, info};

use super::handlers::add_client_key::add_client_key_handler;
use crate::verifysignature;

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub(crate) fn setup(
    config: &crate::config::ServerConfig,
) -> eyre::Result<Option<(&'static str, Router)>> {
    let _config = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin api is disabled");
            return Ok(None);
        }
    };
    let admin_path = "/admin-api";

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);
    let admin_router = Router::new()
        .route("/health", get(health_check_handler))
        .route("/root-key", post(create_root_key_handler))
        .route("/request-challenge", post(request_challenge_handler))
        .route("/install-application", post(install_application_handler))
        .route("/add-client-key", post(add_client_key_handler))
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

pub struct ApiResponse<T: Serialize> {
    pub(crate) payload: T,
}

impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        //TODO add data to response
        let body = serde_json::to_string(&self.payload).unwrap();
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub(crate) status_code: StatusCode,
    pub(crate) message: String,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status_code, self.message)
    }
}

impl Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        let body = json!({ "error": self.message }).to_string();
        axum::http::Response::builder()
            .status(&self.status_code)
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
            if verifysignature::verify_near_signature(
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
        eyre::bail!("Failed to fetch data from the rpc endpoint")
    }
}

pub async fn download_release(release: Release) -> eyre::Result<()> {
    let app_path = "";
    //verify_release(release, blob);
    Ok(())
}

pub async fn verify_release(release: Release, blob: String) {}

pub async fn install_application(application: &String, version: &String) -> eyre::Result<()> {
    download_release(get_release(application, version).await?).await
}

async fn install_application_handler(session: Session, Json(req): Json<InstallApplicationRequest>) {
    install_application(&req.application, &req.version).await;
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
