# Calimero Client Library

A comprehensive, abstract client library for interacting with Calimero APIs. This library provides trait-based abstractions for authentication, storage, and API communication, making it easy to implement different client types (CLI, GUI, headless, etc.) while sharing common functionality.

## Features

- **Abstract Interfaces**: Trait-based design for maximum flexibility
- **Authentication**: Support for various authentication methods
- **Token Storage**: Abstract token management with multiple backends
- **HTTP Client**: Robust HTTP client with retry and error handling
- **Async Support**: Full async/await support throughout
- **Cross-platform**: Works on Windows, macOS, and Linux

## Quick Start

```rust
use calimero_client::{
    ClientAuthenticator, ClientStorage, ConnectionInfo, ClientError
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create your implementations of the traits
    let authenticator = calimero_client::CliAuthenticator::new();
    let storage = calimero_client::MockStorage::new();
    
    // Create a connection
    let connection = ConnectionInfo::new(
        "https://api.calimero.network".parse()?,
        None,
        Some("my-node".to_string()),
        Box::new(authenticator),
        Box::new(storage),
    );
    
    // Use the connection
    let response = connection.get("/health").await?;
    println!("Health: {:?}", response);
    
    Ok(())
}
```

## Architecture

### Core Traits

- **`ClientAuthenticator`**: Handles authentication flows
- **`ClientStorage`**: Manages token persistence
- **`ClientConfig`**: Manages client configuration

### Implementations

- **`CliAuthenticator`**: Browser-based OAuth for interactive use
- **`HeadlessAuthenticator`**: Pre-configured tokens for non-interactive use
- **`ApiKeyAuthenticator`**: Simple API key authentication
- **`FileTokenStorage`**: File-based token storage
- **`MockStorage`**: In-memory storage for testing

### Connection Management

The `ConnectionInfo` struct provides a high-level interface for:
- Making authenticated HTTP requests
- Automatic token refresh
- Retry logic with exponential backoff
- Connection status monitoring

## Usage Examples

### Basic Authentication

```rust
use calimero_client::{CliAuthenticator, ConnectionInfo};

let authenticator = CliAuthenticator::new();
let connection = ConnectionInfo::new(
    "https://api.example.com".parse()?,
    None,
    Some("my-node".to_string()),
    Box::new(authenticator),
    Box::new(storage),
);

// The connection will automatically handle authentication
let data = connection.get("/api/data").await?;
```

### Custom Storage Implementation

```rust
use calimero_client::{ClientStorage, JwtToken};

#[derive(Clone)]
struct DatabaseStorage;

#[async_trait::async_trait]
impl ClientStorage for DatabaseStorage {
    async fn load_tokens(&self, node_name: &str) -> eyre::Result<Option<JwtToken>> {
        // Load from database
        todo!("Implement database loading")
    }
    
    async fn save_tokens(&self, node_name: &str, tokens: &JwtToken) -> eyre::Result<()> {
        // Save to database
        todo!("Implement database saving")
    }
    
    async fn update_tokens(&self, node_name: &str, new_tokens: &JwtToken) -> eyre::Result<()> {
        // Update in database
        todo!("Implement database update")
    }
}
```

### Custom Authenticator

```rust
use calimero_client::{ClientAuthenticator, JwtToken};

#[derive(Clone)]
struct CustomAuthenticator;

#[async_trait::async_trait]
impl ClientAuthenticator for CustomAuthenticator {
    async fn authenticate(&self, api_url: &url::Url) -> eyre::Result<JwtToken> {
        // Implement custom authentication logic
        todo!("Implement custom authentication")
    }
    
    async fn refresh_tokens(&self, refresh_token: &str) -> eyre::Result<JwtToken> {
        // Implement token refresh
        todo!("Implement token refresh")
    }
    
    async fn handle_auth_failure(&self, api_url: &url::Url) -> eyre::Result<JwtToken> {
        // Handle authentication failures
        todo!("Implement auth failure handling")
    }
    
    async fn check_auth_required(&self, api_url: &url::Url) -> eyre::Result<bool> {
        // Check if authentication is required
        Ok(true)
    }
    
    fn get_auth_method(&self) -> &'static str {
        "Custom Authentication"
    }
    
    fn supports_refresh(&self) -> bool {
        true
    }
}
```

## Error Handling

The library provides comprehensive error types:

```rust
use calimero_client::ClientError;

match result {
    Ok(data) => println!("Success: {:?}", data),
    Err(ClientError::Authentication(e)) => {
        eprintln!("Authentication failed: {}", e);
    }
    Err(ClientError::Network(e)) => {
        eprintln!("Network error: {}", e);
    }
    Err(ClientError::Storage(e)) => {
        eprintln!("Storage error: {}", e);
    }
    Err(e) => eprintln!("Other error: {}", e),
}
```

## Configuration

### Connection Configuration

```rust
use calimero_client::{ConnectionConfig, ConnectionInfo};
use std::time::Duration;

let config = ConnectionConfig {
    timeout: Duration::from_secs(60),
    max_retries: 5,
    retry_delay: Duration::from_secs(2),
    auto_refresh: true,
    custom_headers: {
        let mut headers = std::collections::HashMap::new();
        headers.insert("User-Agent".to_string(), "MyApp/1.0".to_string());
        headers
    },
};

let connection = ConnectionInfo::with_config(
    url,
    tokens,
    node_name,
    authenticator,
    storage,
    config,
);
```

### Client Settings

```rust
use calimero_client::ClientSettings;

let settings = ClientSettings {
    request_timeout: 60,
    max_retries: 5,
    retry_delay_ms: 2000,
    use_http2: true,
    user_agent: "MyApp/1.0".to_string(),
};
```

## Testing

The library includes comprehensive testing support:

```rust
use calimero_client::{MockStorage, HeadlessAuthenticator};

#[tokio::test]
async fn test_connection() {
    let authenticator = HeadlessAuthenticator::with_tokens(test_tokens);
    let storage = MockStorage::new();
    
    let connection = ConnectionInfo::new(
        test_url,
        Some(test_tokens),
        Some("test-node".to_string()),
        Box::new(authenticator),
        Box::new(storage),
    );
    
    let status = connection.get_status().await;
    assert!(matches!(status, ConnectionStatus::Authenticated { .. }));
}
```

## Contributing

Contributions are welcome! Please see our [Contributing Guide](../../CONTRIBUTING.md) for details.

## License

This project is licensed under the MIT License - see the [LICENSE](../../LICENSE.md) file for details.

## Changelog

See [CHANGELOG.md](../../CHANGELOG.md) for a list of changes and version history.
