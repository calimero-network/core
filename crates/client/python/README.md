# Calimero Client Python Bindings

[![PyPI version](https://badge.fury.io/py/calimero-client-py-bindings.svg)](https://badge.fury.io/py/calimero-client-py-bindings)
[![Python versions](https://img.shields.io/pypi/pyversions/calimero-client-py-bindings.svg)](https://pypi.org/project/calimero-client-py-bindings/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Documentation](https://img.shields.io/badge/docs-calimero.network-blue.svg)](https://docs.calimero.network)

A high-performance Python client library for Calimero Network APIs, built with PyO3 for native Rust integration.

## 🚀 Features

- **High Performance**: Built with Rust and PyO3 for maximum speed and efficiency
- **Full API Coverage**: Complete access to Calimero Network APIs
- **Async Support**: Full async/await support for non-blocking operations
- **Authentication**: Built-in JWT token handling and authentication modes
- **Error Handling**: Python-friendly error types with detailed messages
- **Type Safety**: Full type hints and mypy support
- **CLI Interface**: Command-line tool for quick API interactions

## 📦 Installation

### From PyPI (Recommended)

```bash
pip install calimero-client-py-bindings
```

### From Source

```bash
# Clone the repository
git clone https://github.com/calimero-network/core.git
cd core/crates/client

# Install in development mode
pip install -e .
```

## 🏗️ Building from Source

### Prerequisites

- Rust toolchain (1.70+)
- Python 3.8+
- maturin

### Build Steps

```bash
# Install maturin
pip install maturin

# Build the package
maturin build --features python

# Install the built wheel
pip install target/wheels/calimero_client_py_bindings-*.whl
```

## 🎯 Quick Start

### Basic Usage

```python
import asyncio
from calimero_client_py_bindings import create_connection, create_client

async def main():
    # Create a connection to Calimero API
    connection = create_connection(
        api_url="https://api.calimero.network",
        node_name="my-node"
    )
    
    # Create a client
    client = create_client(connection)
    
    # Check API health
    response = await connection.get("/health")
    print(f"API Health: {response.status_code}")
    
    # Get supported alias types
    alias_types = await client.get_supported_alias_types()
    print(f"Supported alias types: {alias_types}")

# Run the async function
asyncio.run(main())
```

### Connection Management

```python
from calimero_client_py_bindings import ConnectionInfo, AuthMode

# Create a connection with authentication
connection = ConnectionInfo(
    api_url="https://api.calimero.network",
    node_name="my-node"
)

# Check authentication requirements
auth_mode = connection.detect_auth_mode()
print(f"Authentication required: {auth_mode.value}")

# Make HTTP requests
response = await connection.get("/api/v1/status")
data = await connection.post("/api/v1/data", {"key": "value"})
```

### Error Handling

```python
from calimero_client_py_bindings import ClientError

try:
    response = await connection.get("/api/v1/protected")
except ClientError as e:
    if "Authentication" in str(e):
        print("Authentication required")
    elif "Network" in str(e):
        print("Network error occurred")
    else:
        print(f"Client error: {e}")
```

## 🖥️ Command Line Interface

The package includes a powerful CLI for quick API interactions:

```bash
# Check API health
calimero-client-py health --api-url https://api.calimero.network

# List supported alias types
calimero-client-py aliases --api-url https://api.calimero.network --node-name my-node

# Make a custom request
calimero-client-py request --method GET --endpoint /api/v1/status --api-url https://api.calimero.network

# Verbose output
calimero-client-py health --api-url https://api.calimero.network --verbose
```

## 📚 API Reference

### Core Functions

- `create_connection(api_url: str, node_name: Optional[str]) -> ConnectionInfo`
- `create_client(connection: ConnectionInfo) -> Client`

### Main Classes

#### ConnectionInfo
- `get(endpoint: str) -> Response`
- `post(endpoint: str, data: dict) -> Response`
- `put(endpoint: str, data: dict) -> Response`
- `delete(endpoint: str) -> Response`
- `detect_auth_mode() -> AuthMode`

#### Client
- `get_supported_alias_types() -> List[str]`
- `resolve_alias(alias: str) -> ResolveResponse`

#### Error Types
- `ClientError`: Base error class with subtypes
- `NetworkError`: Network-related errors
- `AuthenticationError`: Authentication failures
- `StorageError`: Storage operation failures

## 🔧 Development

### Setting up Development Environment

```bash
# Clone and setup
git clone https://github.com/calimero-network/core.git
cd core/crates/client

# Install development dependencies
pip install -e ".[dev,test,docs]"

# Run tests
pytest

# Run linting
black src/
isort src/
flake8 src/
mypy src/

# Run type checking
mypy src/
```

### Testing

```bash
# Run all tests
pytest

# Run with coverage
pytest --cov=calimero_client_py_bindings

# Run specific test categories
pytest -m "not slow"
pytest -m integration
pytest -m unit
```

## 📖 Documentation

- [API Reference](https://docs.calimero.network)
- [Examples](https://github.com/calimero-network/core/tree/main/crates/client/src/bindings/python/examples)
- [Contributing Guide](https://github.com/calimero-network/core/blob/main/CONTRIBUTING.md)

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guide](https://github.com/calimero-network/core/blob/main/CONTRIBUTING.md) for details.

### Development Workflow

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Run the test suite
6. Submit a pull request

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](https://github.com/calimero-network/core/blob/main/LICENSE.md) file for details.

## 🆘 Support

- **Documentation**: [docs.calimero.network](https://docs.calimero.network)
- **Issues**: [GitHub Issues](https://github.com/calimero-network/core/issues)
- **Discussions**: [GitHub Discussions](https://github.com/calimero-network/core/discussions)
- **Email**: team@calimero.network

## 🙏 Acknowledgments

- Built with [PyO3](https://pyo3.rs/) for high-performance Python-Rust integration
- Powered by the Calimero Network team and community
- Thanks to all contributors and users

---

**Made with ❤️ by the Calimero Network team**
