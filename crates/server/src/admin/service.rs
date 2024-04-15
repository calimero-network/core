use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::Write;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_primitives::application::ApplicationId;
use calimero_store::Store;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use rand::{thread_rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use tracing::{error, info};

use super::handlers::add_client_key::{add_client_key_handler, parse_api_error};
use super::handlers::fetch_did::fetch_did_handler;
use super::storage::root_key::{add_root_key, RootKey};
use crate::{verifysignature, APPLICATION_ID};

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
    pub application_dir: camino::Utf8PathBuf,
}

pub(crate) struct ServiceState {
    application_dir: camino::Utf8PathBuf,
}

pub(crate) fn setup(
    config: &crate::config::ServerConfig,
    store: Store,
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
        .route("/install-application", post(install_application_handler))
        .layer(Extension(state.clone()))
        .route("/add-client-key", post(add_client_key_handler))
        .route("/did", get(fetch_did_handler))
        .route("/applications", get(fetch_application_handler))
        .layer(Extension(state))
        .layer(session_layer)
        .with_state(store);

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
    State(store): State<Store>,
    Json(req): Json<PubKeyRequest>,
) -> impl IntoResponse {
    let message = "helloworld";
    let app = "me";

    //TODO extract from request
    let application_id = ApplicationId(APPLICATION_ID.to_string());

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
                let result = add_root_key(
                    application_id,
                    &store,
                    RootKey {
                        signing_key: req.public_key.clone(),
                    },
                );

                match result {
                    Ok(_) => {
                        info!("Root key added");
                        (StatusCode::OK, "Root key added")
                    }
                    Err(e) => {
                        error!("Failed to store root key: {}", e);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to store root key",
                        )
                    }
                }
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

pub async fn get_release(application: &str, version: &str) -> eyre::Result<Release> {
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
        return Ok(serde_json::from_slice::<Release>(&result.result)?);
    } else {
        eyre::bail!("Failed to fetch data from the rpc endpoint")
    }
}

pub async fn download_release(
    application: &str,
    release: &Release,
    dir: &camino::Utf8Path,
) -> eyre::Result<()> {
    let base_path = format!("./{}/{}/{}", dir, application, &release.version);
    fs::create_dir_all(&base_path)?;

    let file_path = format!("{}/binary.wasm", base_path);
    let mut file = File::create(&file_path)?;

    let mut response = reqwest::Client::new().get(&release.path).send().await?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk)?;
    }
    let result = hasher.finalize();
    let hash = format!("{:x}", result);

    if let Err(e) = verify_release(&hash, &release.hash).await {
        if let Err(e) = std::fs::remove_file(&file_path) {
            eprintln!("Failed to delete file: {}", e);
        }
        return Err(e.into());
    }

    Ok(())
}

pub async fn verify_release(hash: &str, release_hash: &str) -> eyre::Result<()> {
    if hash != release_hash {
        return Err(eyre::eyre!(
            "Release hash does not match the hash of the downloaded file"
        ));
    }
    Ok(())
}

pub async fn install_application(
    application: &str,
    version: &str,
    dir: &camino::Utf8Path,
) -> eyre::Result<()> {
    let release = get_release(application, version).await?;
    download_release(application, &release, dir).await
}

async fn install_application_handler(
    Extension(state): Extension<Arc<ServiceState>>,
    session: Session,
    Json(req): Json<InstallApplicationRequest>,
) -> impl IntoResponse {
    let result = install_application(&req.application, &req.version, &state.application_dir).await;

    Ok(match result {
        Ok(()) => (StatusCode::OK, "Application Installed"),
        Err(err) => return Err(parse_api_error(err)),
    }
    .into_response())
}

fn get_latest_application_path(dir: &camino::Utf8Path, application_id: &str) -> Option<String> {
    let application_base_path = dir.join(application_id.to_string());

    if let Ok(entries) = fs::read_dir(&application_base_path) {
        let mut versions_with_binary = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let entry_path = entry.path();

                let version = {
                    semver::Version::parse(entry_path.file_name()?.to_string_lossy().as_ref())
                        .ok()?
                };

                let binary_path = entry_path.join("binary.wasm");
                binary_path.exists().then_some((version, entry_path))
            })
            .collect::<Vec<_>>();

        versions_with_binary.sort_by(|a, b| b.0.cmp(&a.0));

        if let Some((_, path)) = versions_with_binary.first() {
            Some(path.to_string_lossy().into_owned())
        } else {
            None
        }
    } else {
        None
    }
}

async fn fetch_application_handler(
    Extension(state): Extension<Arc<ServiceState>>,
    session: Session,
) -> impl IntoResponse {
    if let Ok(entries) = fs::read_dir(&state.application_dir) {
        let mut applications: HashMap<String, String> = HashMap::new();

        entries.filter_map(|entry| entry.ok()).for_each(|entry| {
            if let Some(file_name) = entry.file_name().to_str() {
                let latest_version =
                    get_latest_application_path(&state.application_dir, &file_name);
                if let Some(latest_version) = latest_version {
                    let app_name = file_name.to_string();
                    if let Some(version) = latest_version.rsplit('/').next() {
                        applications.insert(app_name, version.to_string());
                    }
                }
            }
        });
        let response_body = json!({
            "apps": applications
        });
        return ApiResponse {
            payload: response_body.to_string(),
        }
        .into_response();
    } else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read application directory",
        )
            .into_response();
    }
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
