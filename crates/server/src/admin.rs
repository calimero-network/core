use axum::{response::IntoResponse, routing::{get, post, put}, Json, Router};
use rand::{thread_rng, RngCore};
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use base64::{engine::general_purpose::STANDARD, Engine};

use crate::http::StatusCode;

const CHALLENGE_KEY: &str = "challenge";

async fn health_check() -> Json<&'static str> {
    Json("{\"status\":\"ok\"}")
}

async fn request_challenge(session: Session) -> impl IntoResponse {
    if let Some(challenge) = session.get(CHALLENGE_KEY).await.unwrap_or(None) {
        // Challenge already exists in session, return it
        (StatusCode::OK, challenge)
    } else {
        // No challenge in session, generate a new one
        let challenge = generate_challenge();  // Implement this function
        session.insert("challenge", &challenge).await.unwrap();  // Insert the new challenge into the session
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

async fn create_root_key() -> Json<&'static str> {
    Json("{\"status\":\"ok\"}")
}


#[derive(serde::Deserialize)]
struct PubKeyRequest {
    public_key: String,
    signature: String,  // Assuming the signature is part of the request body
    // Include other necessary fields
}

async fn add_root_pubkey(session: Session, Json(req): Json<PubKeyRequest>) -> impl IntoResponse {
    // Retrieve the challenge from the session
    // if let Some(challenge) = session.get(CHALLENGE_KEY).await.unwrap_or(None) {
        // Verify the signature against the challenge
        // if verify_signature(&challenge, &req.signature, &req.public_key) {  // Implement this function
        //     // Signature verification successful, proceed with adding the public key
        //     // ...
        //     (StatusCode::OK, "{\"status\":\"public key added\"}")
        // } else {
            // Signature verification failed
            // println!("challenge: {}", challenge);
            (StatusCode::UNAUTHORIZED, "{\"status\":\"invalid signature\"}")
        // }
    // } else {
    //     // No challenge found in session
    //     (StatusCode::BAD_REQUEST, "{\"status\":\"challenge not found\"}")
    // }
}

pub fn admin_router() -> Router {
    Router::new()
    .route("/health", get(health_check))
    .route("/node-key", put(add_root_pubkey))
}

pub fn bootstrap_router() -> Router {
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);
    Router::new()
    .route("/health", get(health_check))
    .route("/node-key", post(create_root_key))
    .route("/request-challenge", post(request_challenge))
    .layer(session_layer)
}