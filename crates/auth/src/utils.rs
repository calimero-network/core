use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};

/// Challenge request
#[derive(Debug, Deserialize)]
pub struct ChallengeRequest {
    pub provider: String,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
}

/// Challenge response
#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    pub message: String,
    pub timestamp: u64,
    pub network: String,
    pub rpc_url: String,
    pub wallet_url: String,
    pub redirect_uri: String,
}

/// Generate a random challenge
pub fn generate_random_challenge() -> String {
    let mut rng = thread_rng();
    let random_bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
    STANDARD.encode(random_bytes)
}
