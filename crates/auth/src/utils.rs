use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

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

/// Basic metrics collector for auth operations
#[derive(Debug, Clone)]
pub struct AuthMetrics {
    /// Total number of authentication attempts
    auth_attempts: Arc<AtomicU64>,
    /// Number of successful authentications
    auth_successes: Arc<AtomicU64>,
    /// Number of failed authentications
    auth_failures: Arc<AtomicU64>,
    /// Average authentication duration in milliseconds
    auth_duration_ms: Arc<RwLock<(u64, u64)>>, // (total_ms, count)
    /// Authentication failures by error type
    auth_failures_by_type: Arc<RwLock<HashMap<String, u64>>>,
    /// Number of token refreshes
    token_refreshes: Arc<AtomicU64>,
    /// Number of token revocations
    token_revocations: Arc<AtomicU64>,
    /// Service start time
    start_time: Arc<Instant>,
}

impl AuthMetrics {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            auth_attempts: Arc::new(AtomicU64::new(0)),
            auth_successes: Arc::new(AtomicU64::new(0)),
            auth_failures: Arc::new(AtomicU64::new(0)),
            auth_duration_ms: Arc::new(RwLock::new((0, 0))),
            auth_failures_by_type: Arc::new(RwLock::new(HashMap::new())),
            token_refreshes: Arc::new(AtomicU64::new(0)),
            token_revocations: Arc::new(AtomicU64::new(0)),
            start_time: Arc::new(Instant::now()),
        }
    }

    /// Start tracking an authentication attempt and return a timer
    pub fn start_auth_attempt(&self) -> AuthTimer {
        self.auth_attempts.fetch_add(1, Ordering::Relaxed);
        AuthTimer {
            start_time: Instant::now(),
            metrics: self.clone(),
        }
    }

    /// Record a successful authentication with the given duration
    pub async fn record_auth_success(&self, duration_ms: u64) {
        self.auth_successes.fetch_add(1, Ordering::Relaxed);
        
        // Update average duration
        let mut data = self.auth_duration_ms.write().await;
        let (total, count) = *data;
        *data = (total + duration_ms, count + 1);
    }

    /// Record a failed authentication with the given duration and error type
    pub async fn record_auth_failure(&self, duration_ms: u64, error_type: &str) {
        self.auth_failures.fetch_add(1, Ordering::Relaxed);
        
        // Update failure by type count
        let mut failures = self.auth_failures_by_type.write().await;
        *failures.entry(error_type.to_string()).or_insert(0) += 1;
        
        // Update average duration
        let mut data = self.auth_duration_ms.write().await;
        let (total, count) = *data;
        *data = (total + duration_ms, count + 1);
    }

    /// Record a token refresh
    pub fn record_token_refresh(&self) {
        self.token_refreshes.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a token revocation
    pub fn record_token_revocation(&self) {
        self.token_revocations.fetch_add(1, Ordering::Relaxed);
    }

    /// Get all metrics as a JSON-serializable map
    pub async fn get_metrics(&self) -> HashMap<String, serde_json::Value> {
        let mut metrics = HashMap::new();
        
        // Basic counts
        metrics.insert(
            "auth_attempts".to_string(), 
            serde_json::Value::Number(serde_json::Number::from(self.auth_attempts.load(Ordering::Relaxed)))
        );
        metrics.insert(
            "auth_successes".to_string(), 
            serde_json::Value::Number(serde_json::Number::from(self.auth_successes.load(Ordering::Relaxed)))
        );
        metrics.insert(
            "auth_failures".to_string(), 
            serde_json::Value::Number(serde_json::Number::from(self.auth_failures.load(Ordering::Relaxed)))
        );
        
        // Calculate success rate
        let attempts = self.auth_attempts.load(Ordering::Relaxed);
        let successes = self.auth_successes.load(Ordering::Relaxed);
        let success_rate = if attempts > 0 {
            (successes as f64 / attempts as f64) * 100.0
        } else {
            0.0
        };
        metrics.insert(
            "auth_success_rate".to_string(),
            serde_json::Value::Number(serde_json::Number::from_f64(success_rate).unwrap_or(serde_json::Number::from(0)))
        );
        
        // Average duration
        let data = self.auth_duration_ms.read().await;
        let (total, count) = *data;
        let avg_duration = if count > 0 { total / count } else { 0 };
        metrics.insert(
            "auth_avg_duration_ms".to_string(),
            serde_json::Value::Number(serde_json::Number::from(avg_duration))
        );
        
        // Failures by type
        let failures = self.auth_failures_by_type.read().await;
        let failures_json = serde_json::to_value(&*failures).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        metrics.insert("auth_failures_by_type".to_string(), failures_json);
        
        // Token operations
        metrics.insert(
            "token_refreshes".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.token_refreshes.load(Ordering::Relaxed)))
        );
        metrics.insert(
            "token_revocations".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.token_revocations.load(Ordering::Relaxed)))
        );
        
        metrics
    }

    /// Get service uptime in seconds
    pub fn get_uptime_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }
}

/// Authentication timer for tracking authentication duration
pub struct AuthTimer {
    start_time: Instant,
    metrics: AuthMetrics,
}

impl AuthTimer {
    /// Record a successful authentication
    pub async fn success(self) {
        let duration = self.start_time.elapsed();
        self.metrics.record_auth_success(duration.as_millis() as u64).await;
    }
    
    /// Record a failed authentication
    pub async fn failure(self, error_type: &str) {
        let duration = self.start_time.elapsed();
        self.metrics.record_auth_failure(duration.as_millis() as u64, error_type).await;
    }
}
