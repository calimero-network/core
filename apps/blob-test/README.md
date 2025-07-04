# Blob API Test Application

This application demonstrates the complete Calimero Blob API implementation, showcasing both REST API endpoints and JSON RPC methods for blob storage and retrieval.

## What This Demonstrates

This is a **complete implementation** of blob functionality for Calimero, including:

### Backend Components
- **Runtime Host Functions**: WASM-accessible blob operations (`store_blob`, `load_blob`)
- **SDK API**: Developer-friendly blob functions for applications
- **REST Endpoints**: Direct file upload/download via HTTP
- **JSON RPC Methods**: Application-context blob management
- **Node Integration**: Integration with Calimero's execution environment

### Frontend Components  
- **React Interface**: Modern web UI for testing all blob operations
- **API Integration**: Complete implementation of both REST and JSON RPC clients
- **File Management**: Upload, registration, reading, and download workflows

## Blob API Workflow

1. **Upload** (REST): `POST /admin-api/blobs/upload` → Get blob ID
2. **Register** (JSON RPC): Associate blob with human-readable name  
3. **Read** (JSON RPC): Retrieve blob data by name
4. **Download** (REST): Download blob directly by ID

## Running the Demo

```bash
# Build the backend
./build.sh

# Start the frontend  
cd frontend
npm install
npm start
```

## Implementation Structure

This implementation spans multiple crates:
- `crates/runtime/` - Host function implementation
- `crates/sdk/` - Developer API
- `crates/server/` - REST endpoints  
- `crates/node/` - Node integration
- `crates/context/` - Context execution support

Each component is designed for focused review while maintaining integration.

## Testing

The application provides comprehensive testing of:
- File upload/download workflows
- Blob registration and management
- Error handling and edge cases
- Integration between REST and JSON RPC APIs

This serves as both a demo and integration test for the complete blob API implementation. 