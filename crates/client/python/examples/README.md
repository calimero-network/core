# Calimero Client Python Bindings Examples

This directory contains example scripts demonstrating how to use the Calimero client Python bindings.

## Examples

### 1. `simple_example.py` - Basic Connection Test
A simple example that demonstrates basic connection setup without asyncio.

**Features:**
- Creates a connection to `localhost:2528`
- No authentication required
- Synchronous execution
- Easy to run and test

**Usage:**
```bash
cd python/examples
python simple_example.py
```

### 2. `example.py` - Full Async Example
A comprehensive example showing async usage and error handling.

**Features:**
- Async/await pattern
- Connection to `localhost:2528`
- Error handling and graceful shutdown
- Future-ready for health endpoint implementation

**Usage:**
```bash
cd python/examples
python example.py
```

### 3. `health_check_example.py` - Practical HTTP Testing
A practical example that makes actual HTTP requests to test connectivity.

**Features:**
- Real HTTP requests to `localhost:2528`
- Health endpoint testing (`/health`)
- Fallback connectivity testing (`/`)
- Proper error handling and status code checking
- Useful for testing if a Calimero node is running

**Usage:**
```bash
cd python/examples
python health_check_example.py
```

## Prerequisites

Before running the examples, make sure you have:

1. **Built the package:**
   ```bash
   cd crates/client/python
   ./build.sh
   ```

2. **Installed the package:**
   ```bash
   pip install ../../target/wheels/calimero_client_py_bindings-*.whl
   ```

3. **A running Calimero node** on `localhost:2528` (optional for basic testing)

## Configuration

The examples are configured to connect to:
- **URL:** `http://localhost:2528`
- **Timeout:** 30 seconds
- **Authentication:** None (for local development)

## Customization

You can modify the examples to:
- Connect to different endpoints
- Add authentication
- Implement actual API calls
- Handle different response types

## Troubleshooting

### Import Error
If you get an import error, make sure the package is installed:
```bash
pip install ../../target/wheels/calimero_client_py_bindings-*.whl
```

### Connection Error
If you get a connection error:
- Check if a Calimero node is running on port 2528
- Verify the port is accessible
- Check firewall settings

### Build Error
If the build fails:
- Ensure you have Rust and maturin installed
- Check that all dependencies are available
- Run `./build.sh` from the `python/` directory

## Next Steps

After running the examples successfully:
1. Implement actual API endpoint calls
2. Add proper error handling for your use case
3. Integrate with your application
4. Add authentication if needed
5. Handle different response types and status codes
