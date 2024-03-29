use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{AccountId, BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use rand::{thread_rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{from_slice, json};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use tracing::{error, info};


use crate::verifysignature;

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub(crate) fn service(
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
        .route("/get-applications", get(get_installed_applications_handler))
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
    let curl = "http://127.0.0.1:2428/admin/confirm-wallet";

    match session.get::<String>(CHALLENGE_KEY).await.ok().flatten() {
        Some(challenge) => {
            if verifysignature::verify_signature(
                &challenge,
                message,
                app,
                curl,
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
pub struct Package {
    name: String,
    description: String,
    repository: String,
    owner: AccountId
}

pub async fn get_application_metadata(application: String, version: String) -> eyre::Result<()>{
    let client = JsonRpcClient::connect("https://rpc.testnet.near.org");

    let request = methods::query::RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::CallFunction {
            account_id: "calimero-package-manager.testnet".parse()?,
            method_name: "get_release".to_string(),
            args: FunctionArgs::from(
                json!({
                    "application": application,
                    "version": version
                })
                .to_string()
                .into_bytes(),
            ),
        },
    };

    let response = client.call(request).await?;
    if let QueryResponseKind::CallResult(result) = response.kind {
        println!("{:#?}", from_slice::<Package>(&result.result)?);
    }

    Ok(())
}

pub async fn download_application(application: String, version: String) -> eyre::Result<()> {
    let app_path = "";
    Ok(())
}

pub async fn verify_application(application: String, version: String, blob: String) {

}

pub fn install_application(application: String, version: String) -> eyre::Result<()> {
    get_application_metadata(application, version);

    //let blob = download_application((application), version);

    //verify_application(blob)
    Ok(())
}

pub fn get_installed_applications() {

}

async fn install_application_handler(session: Session) {
    if let (Some(application), Some(version)) = (
            session.get::<String>("application").await.ok().flatten(),
            session.get::<String>("version").await.ok().flatten()
        ) {
            install_application(application, version);
    }
}

async fn get_installed_applications_handler(session: String) -> impl IntoResponse {
    (StatusCode::OK, "alive")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PubKeyRequest {
    account_id: String,
    public_key: String,
    signature: String,
}
