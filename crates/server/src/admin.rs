use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{thread_rng, RngCore};
use serde_json::to_string_pretty;
use tower_sessions::Session;
use tower_sessions::{MemoryStore, SessionManagerLayer};

pub const CHALLENGE_KEY: &str = "challenge";

pub async fn request_challenge_handler(session: Session) -> impl IntoResponse {
    if let Some(challenge) = session.get(CHALLENGE_KEY).await.unwrap_or(None) {
        (StatusCode::OK, challenge)
    } else {
        // No challenge in session, generate a new one
        let challenge = generate_challenge();
        session.insert("challenge", &challenge).await.unwrap();
        (StatusCode::OK, challenge)
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

async fn health_check() -> Json<&'static str> {
    Json("{\"status\":\"ok\"}")
}

async fn create_root_key(session: Session, Json(req): Json<PubKeyRequest>) -> impl IntoResponse {
    let pretty_json = to_string_pretty(&req).unwrap_or_else(|_| "Failed to serialize".into());

    println!("Pretty printed JSON body:\n{}", pretty_json);

    // Retrieve the challenge from the session
    if let Some(challenge) = session.get::<String>(CHALLENGE_KEY).await.unwrap_or(None) {
        // TODO: Verify the challenge
        // TODO: Verify the signature
        // TODO: Create DidDocument
        println!("Challenge: {:?}", challenge);
        (StatusCode::OK, Json("{\"status\":\"Root key created\"}"))
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json("{\"status\":\"No challenge found\"}"),
        )
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PubKeyRequest {
    account_id: String,
    public_key: String,
    signature: String,
}

pub fn admin_router() -> Router {
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);
    Router::new()
        .route("/health", get(health_check))
        .route("/node-key", post(create_root_key))
        .route("/request-challenge", post(request_challenge_handler))
        .layer(session_layer)
}
