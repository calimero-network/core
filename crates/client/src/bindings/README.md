# Calimero Client Python Bindings

This directory contains Python bindings for the Calimero client library, built using [PyO3](https://pyo3.rs/).

## Features

The Python bindings provide access to the core Calimero client functionality:

- **Connection Management**: Create and manage connections to Calimero APIs
- **HTTP Operations**: GET, POST, DELETE requests with automatic authentication
- **Client Operations**: High-level client for Calimero operations (framework ready)
- **Authentication**: JWT token handling and authentication mode detection
- **Error Handling**: Python-friendly error types with detailed messages

## Installation

### Prerequisites

- Rust toolchain (1.70+)
- Python 3.8+
- Cargo with the `python` feature enabled

### Building

To build the Python bindings, you need to enable the `python` feature:

```bash
# From the workspace root
cargo build --features python

# Or specifically for the client crate
cargo build -p calimero-client --features python

# Or use the provided build script
./build_python.sh
```

### Python Package

The bindings are currently designed to be used as a Rust library with Python bindings. To create a proper Python package, you would need to:

1. Set up `maturin` or `setuptools-rust`
2. Configure `pyproject.toml`
3. Build and distribute via PyPI

## Usage

### Basic Example

```python
from calimero_client import create_connection, create_client

# Create a connection to a Calimero API
connection = create_connection(
    api_url="https://api.calimero.network",
    node_name="my-node"
)

# Create a client
client = create_client(connection)

# Make requests
response = connection.get("/health")
aliases = client.get_supported_alias_types()
```

### Connection Management

```python
from calimero_client import ConnectionInfo, AuthMode

# Create a connection
connection = ConnectionInfo(
    api_url="https://api.calimero.network",
    node_name="my-node"
)

# Check authentication requirements
auth_mode = connection.detect_auth_mode()
print(f"Authentication required: {auth_mode.value}")

# Make HTTP requests
response = connection.get("/api/v1/status")
data = connection.post("/api/v1/data", {"key": "value"})
result = connection.delete("/api/v1/resource/123")
```

### Client Operations

```python
from calimero_client import Client

# Create a client from a connection
client = Client(connection)

# Get supported alias types
alias_types = client.get_supported_alias_types()
print(f"Supported types: {alias_types}")

# Note: Advanced alias operations are framework-ready but not yet implemented
# These will return NotImplementedError for now:
# - client.resolve_alias("alias", "context", None)
# - client.create_alias("alias", "context", value, None)
# - client.delete_alias("alias", "context", None)
# - client.list_aliases("context", None)
# - client.lookup_alias("alias", "context", None)
```

### JWT Token Management

```python
from calimero_client import JwtToken
import time

# Create a token
token = JwtToken(
    access_token="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
    refresh_token="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
    expires_at=int(time.time()) + 3600  # 1 hour from now
)

# Check token status
print(f"Access token: {token.access_token[:20]}...")
print(f"Refresh token: {token.refresh_token[:20] if token.refresh_token else 'None'}...")
print(f"Expires at: {token.expires_at}")
print(f"Is expired: {token.is_expired()}")
```

### Error Handling

```python
from calimero_client import ClientError

try:
    response = connection.get("/nonexistent")
except ClientError as e:
    print(f"Client error: {e.error_type}")
    print(f"Message: {e.message}")
    
    if e.error_type == "Network":
        print("Network-related error occurred")
    elif e.error_type == "Authentication":
        print("Authentication failed")
    elif e.error_type == "Storage":
        print("Storage operation failed")
    elif e.error_type == "Internal":
        print("Internal error occurred")
```

## API Reference

### ConnectionInfo

The `ConnectionInfo` class provides connection management and basic HTTP operations:

- **`api_url`**: Property that returns the API URL as a string
- **`node_name`**: Property that returns the node name
- **`get(url, headers=None)`**: Perform HTTP GET request
- **`post(url, data=None, headers=None)`**: Perform HTTP POST request  
- **`delete(url, headers=None)`**: Perform HTTP DELETE request
- **`detect_auth_mode()`**: Detect authentication mode required by the server

### Client

The `Client` class provides access to all Calimero API operations. **Note**: Many methods currently return `NotImplementedError` and are placeholders for future implementation.

#### Fully Implemented Methods
- **`get_api_url()`**: Returns the API URL as a string
- **`get_application(app_id)`**: Get application information (parses ApplicationId from string)
- **`uninstall_application(app_id)`**: Uninstall application (parses ApplicationId from string)
- **`delete_blob(blob_id)`**: Delete blob (parses BlobId from string)
- **`get_blob_info(blob_id)`**: Get blob information (parses BlobId from string)
- **`get_proposal(context_id, proposal_id)`**: Get proposal (parses ContextId and Hash from strings)
- **`get_proposal_approvers(context_id, proposal_id)`**: Get proposal approvers (parses ContextId and Hash from strings)
- **`list_proposals(context_id, args)`**: List proposals (parses ContextId from string, uses default args)
- **`get_context(context_id)`**: Get context (parses ContextId from string)
- **`delete_context(context_id)`**: Delete context (parses ContextId from string)
- **`sync_context(context_id)`**: Sync context (parses ContextId from string)
- **`get_context_storage(context_id)`**: Get context storage (parses ContextId from string)
- **`get_context_identities(context_id)`**: Get context identities (parses ContextId from string, defaults to all identities)
- **`get_context_client_keys(context_id)`**: Get context client keys (parses ContextId from string)
- **`list_applications()`**: Lists all applications (returns JSON data)
- **`list_blobs()`**: Lists all blobs (returns JSON data)
- **`generate_context_identity()`**: Generates a new context identity (returns JSON data)
- **`get_peers_count()`**: Gets the current peer count (returns JSON data)
- **`list_contexts()`**: Lists all contexts (returns JSON data)

#### Placeholder Methods (Return NotImplementedError)
- **`install_dev_application(request)`**: Install development application (requires InstallDevApplicationRequest parsing)
- **`install_application(request)`**: Install application (requires InstallApplicationRequest parsing)
- **`execute_jsonrpc(request)`**: Execute JSON-RPC request (requires Request parsing)
- **`grant_permissions(context_id, request)`**: Grant permissions (requires capabilities parsing)
- **`revoke_permissions(context_id, request)`**: Revoke permissions (requires capabilities parsing)
- **`invite_to_context(request)`**: Invite to context (requires InviteToContextRequest parsing)
- **`update_context_application(context_id, request)`**: Update context application (requires UpdateContextApplicationRequest parsing)
- **`create_context(request)`**: Create context (requires CreateContextRequest parsing)
- **`join_context(request)`**: Join context (requires JoinContextRequest parsing)

#### Alias Management Methods
- **`get_supported_alias_types()`**: Returns list of supported alias types
- **`resolve_alias(alias, scope)`**: Resolve alias (placeholder)
- **`create_alias(alias, value, scope)`**: Create alias (placeholder)
- **`delete_alias(alias, scope)`**: Delete alias (placeholder)
- **`list_aliases(scope)`**: List aliases (placeholder)
- **`lookup_alias(alias, scope)`**: Lookup alias (placeholder)

### JwtToken

The `JwtToken` class represents JWT authentication tokens:

- **`access_token`**: The access token string
- **`refresh_token`**: The refresh token string
- **`expires_at`**: Expiration timestamp (Unix timestamp)
- **`token_type`**: Token type (e.g., "Bearer")

### AuthMode

The `AuthMode` enum represents different authentication modes:

- **`NoAuth`**: No authentication required
- **`BasicAuth`**: Basic username/password authentication
- **`JwtAuth`**: JWT token-based authentication

### ClientError

The `ClientError` class represents client operation errors:

- **`NetworkError`**: Network-related errors
- **`AuthenticationError`**: Authentication failures
- **`SerializationError`**: Data serialization/deserialization errors
- **`ValidationError`**: Input validation errors

### Functions

- **`create_connection(api_url, node_name, username=None, password=None, storage_path=None)`**: Create a new connection
- **`create_client(connection)`**: Create a new client instance

## Architecture

The Python bindings use PyO3 to create Python classes that wrap the Rust types:

1. **Rust Backend**: All business logic runs in Rust for performance
2. **Python Interface**: Clean Python API that feels native
3. **Async Bridge**: Tokio runtime integration for async operations
4. **Type Safety**: Full type safety between Rust and Python
5. **Error Handling**: Python exceptions mapped from Rust errors

## Development

### Adding New Bindings

To add new Python bindings:

1. Create a new `Py*` struct in `python.rs`
2. Implement `#[pymethods]` for the struct
3. Add the class to the `#[pymodule]` function
4. Update the example and documentation

### Implementing Client Methods

To implement a currently placeholder client method:

1. **Simple methods**: Convert Rust types to JSON using `serde_json::to_value()` then use `into_py(py)`
2. **Complex methods**: Create helper functions to parse Python inputs to Rust types
3. **Error handling**: Use `PyErr::new::<pyo3::exceptions::PyRuntimeError, _>()` for Rust errors
4. **Async operations**: Use `self.runtime.block_on(async move { ... })` to bridge async Rust to sync Python

### Testing

```bash
# Build with Python feature
cargo build --features python

# Run the Python example
python3 examples/python_example.py
```

### Dependencies

The Python bindings require:
- `pyo3` crate with `auto-initialize` feature
- `tokio` runtime for async operations
- `serde_json` for JSON serialization

## Limitations

- **Async Operations**: All async operations are blocked on the Tokio runtime using `block_on`
- **Token Storage**: Current implementation uses simplified file-based storage for Python
- **Error Types**: Some Rust-specific error details may be simplified in Python exceptions
- **Performance**: Python GIL may impact concurrent operations
- **Client Operations**: Most client operations (19 methods) are fully implemented, with 10+ methods returning `NotImplementedError` as placeholders
- **Type Conversion**: Complex Rust types with generic constraints are challenging to expose directly to Python
- **Memory Usage**: Large responses are converted to JSON before Python conversion, which may not be memory efficient

## Future Improvements

The Python bindings are currently in a foundational state with the following planned enhancements:

### Phase 1: Core Client Methods (Current Priority)
- **Implement full Client methods**: Currently 19 methods are fully implemented (including `get_application`, `uninstall_application`, `delete_blob`, `get_blob_info`, `get_proposal`, `get_proposal_approvers`, `list_proposals`, `get_context`, `delete_context`, `sync_context`, `get_context_storage`, `get_context_identities`, `get_context_client_keys`, `list_applications`, `list_blobs`, `generate_context_identity`, `get_peers_count`, `list_contexts`). The remaining 10+ methods return `NotImplementedError` and need proper implementation.
- **Type conversion utilities**: Helper functions for converting between Python types and Rust types (e.g., `ApplicationId`, `ContextId`, `BlobId`, `Hash`) are now implemented and working.
- **Request/Response parsing**: Implement proper parsing for complex request types like `InstallApplicationRequest`, `CreateContextRequest`, etc.

### Phase 2: Enhanced Functionality
- **Async support**: Convert synchronous Python methods to use Python's `asyncio` for better performance and non-blocking operations.
- **Batch operations**: Add methods for batch processing of multiple requests.
- **Streaming responses**: Implement streaming for large data operations like blob downloads.
- **Connection pooling**: Add connection pooling and reuse for better performance.

### Phase 3: Advanced Features
- **Error handling improvements**: Create more specific Python exception types that map to Rust error variants.
- **Logging integration**: Add Python logging integration with configurable log levels.
- **Configuration management**: Python-native configuration management with environment variable support.
- **Testing framework**: Comprehensive test suite with mocked responses and integration tests.

### Phase 4: Developer Experience
- **Type hints**: Add comprehensive Python type hints for better IDE support and static analysis.
- **Documentation**: Generate detailed API documentation with examples for each method.
- **CLI tools**: Python command-line tools for common operations.
- **Jupyter notebook integration**: Examples and utilities for data analysis workflows.

### Current Limitations
- **Generic type constraints**: Many Rust methods use generic types with complex trait bounds that are challenging to expose directly to Python.
- **Async to sync conversion**: Current implementation uses `tokio::runtime::Runtime::block_on` which may not be optimal for all use cases.
- **Memory management**: Large response objects are converted to JSON before Python conversion, which may not be memory efficient for very large datasets.
- **Complex request types**: Methods that require parsing complex Rust request types (like `InstallApplicationRequest`, `CreateContextRequest`) are still placeholders.

### Implementation Notes
- The bindings use a hybrid approach: simple methods that return basic types are fully implemented, while complex methods that require parsing custom Rust types return `NotImplementedError` with descriptive messages.
- Response types are converted to JSON using `serde_json::to_value()` before converting to Python objects, ensuring compatibility with the existing `IntoPyJson` trait.
- The `PyClient` class maintains an internal `Arc<ConnectionInfo>` and `tokio::Runtime` for managing async operations and shared state.
