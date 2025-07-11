# Blob Test Frontend

A React frontend application for testing the Calimero Blob API implementation.

## Setup

1. **Install dependencies:**
   ```bash
   cd apps/blob-test/frontend
   pnpm install
   ```

2. **Start the development server:**
   ```bash
   pnpm dev
   ```
   
   The app will be available at `http://localhost:5174`

3. **Build for production:**
   ```bash
   pnpm build
   ```

## Prerequisites

Before using this frontend, you need to:

1. **Install and run the blob-test backend:**
   ```bash
   # From the blob-test directory
   cd apps/blob-test
   ./build.sh
   ```

2. **Start the Calimero node with your context:**
   - Make sure your Calimero node is running
   - Install the blob-test app in your context
   - Note your context ID and context identity

3. **Configure the frontend:**
   - When you first open the app, you'll be prompted to log in through the Calimero client
   - Provide your context ID and context identity when prompted

## Features

### 🗂️ Blob Operations
- **Create Blob from Text:** Enter text content and create a blob
- **Create Blob from File:** Upload any file and create a blob from its contents
- **Read Blob:** Retrieve and display blob contents by name
- **List Blobs:** View all stored blobs with their IDs

### 🧪 Testing
- **Basic Operations Test:** Runs the built-in test that creates, reads, and verifies a blob
- **Multipart Test:** Tests blob creation with multiple data chunks
- **Statistics:** View blob count and registry size

### 📊 Real-time Features
- **Live Output Log:** See all operations and their results in real-time
- **Auto-refresh:** Blob list and stats update automatically after operations
- **Error Handling:** Clear error messages and status indicators

## Usage Flow

1. **Start the app** and log in with your Calimero credentials
2. **Create some blobs** using either text input or file upload
3. **Read blobs** to verify they were stored correctly
4. **Run tests** to validate your blob API implementation
5. **Monitor the output log** to see detailed operation results

## API Methods Tested

This frontend tests all the blob API methods implemented in your Rust backend:

- `create_blob(name, data)` - Creates a new blob
- `read_blob(name)` - Reads blob data by name
- `list_blobs()` - Lists all stored blobs
- `test_basic_operations()` - Runs built-in basic test
- `test_multipart_blob(chunks)` - Tests multipart blob creation
- `get_stats()` - Gets storage statistics

## Development

The app is structured as:
- `src/api/` - API interfaces and data sources
- `src/pages/` - React components for the UI
- `src/App.tsx` - Main app component with Calimero authentication
- `src/index.tsx` - App entry point

The frontend automatically handles:
- Authentication with Calimero
- RPC communication with your backend
- Error handling and user feedback
- Data serialization (text/files to byte arrays) 