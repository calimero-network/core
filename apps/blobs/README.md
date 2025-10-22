# File Sharing App - Blob API Implementation

A minimal demonstration of the Calimero blob API for building decentralized file sharing applications.

## Overview

This application demonstrates how to use the Calimero blob storage API to build a simple file sharing backend. It shows the core patterns for:

- **Blob Storage**: Storing files as blobs with metadata
- **Network Announcement**: Making blobs discoverable across the network
- **File Management**: Upload, delete, list, and search files
- **Base58 Encoding**: Safe serialization of binary blob IDs

## Key Concepts

### Blob IDs

Blobs are identified by 32-byte IDs. For safe transmission and storage:

- **Internal**: `[u8; 32]` - Raw bytes for storage and network operations
- **External**: `String` - Base58-encoded for JSON serialization and API responses

### Blob Announcement

When a file is uploaded, its blob is announced to the network:

```rust
env::blob_announce_to_context(&blob_id, &current_context)
```

This allows other nodes in the same context to:

1. Discover the blob exists
2. Request it from peers who have it
3. Build a distributed storage network

## Data Structures

### FileRecord

Stores metadata about each uploaded file:

```rust
pub struct FileRecord {
    pub id: String,              // Unique file ID (e.g., "file_0")
    pub name: String,            // Human-readable name
    pub blob_id: [u8; 32],       // Binary blob ID
    pub size: u64,               // File size in bytes
    pub mime_type: String,       // Content type
    pub uploaded_by: String,     // Uploader's ID
    pub uploaded_at: u64,        // Timestamp
}
```

### FileShareState

Application state using Calimero storage collections:

```rust
pub struct FileShareState {
    pub owner: String,
    pub files: UnorderedMap<String, FileRecord>,  // ID -> FileRecord
    pub file_counter: u64,                        // For generating unique IDs
}
```

## API Methods

### Upload a File

```rust
upload_file(
    name: String,
    blob_id_str: String,  // Base58-encoded blob ID
    size: u64,
    mime_type: String
) -> Result<String, String>
```

**Process:**

1. Parse blob ID from base58
2. Generate unique file ID
3. **Announce blob to network** (key blob API usage)
4. Store file metadata
5. Emit event

**Example:**

```rust
let file_id = state.upload_file(
    "document.pdf".to_string(),
    "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
    1024000,
    "application/pdf".to_string()
)?;
```

### Delete a File

```rust
delete_file(file_id: String) -> Result<(), String>
```

Removes the file record from storage and emits a deletion event.

### List All Files

```rust
list_files() -> Result<Vec<FileRecord>, String>
```

Returns all stored files with their metadata.

### Get Specific File

```rust
get_file(file_id: String) -> Result<FileRecord, String>
```

Retrieves a single file's metadata by ID.

### Get Blob ID

```rust
get_blob_id_b58(file_id: String) -> Result<String, String>
```

Returns the base58-encoded blob ID for a file (useful for downloading).

### Search Files

```rust
search_files(query: String) -> Result<Vec<FileRecord>, String>
```

Case-insensitive search by filename.

### Statistics

```rust
get_stats() -> Result<String, String>
get_total_files_size() -> Result<u64, String>
```

Get usage statistics and total storage.

## Events

The application emits events for important operations:

```rust
pub enum FileShareEvent {
    FileUploaded {
        id: String,
        name: String,
        size: u64,
        uploader: String,
    },
    FileDeleted {
        id: String,
        name: String,
    },
}
```

These events can be subscribed to by clients for real-time updates.

## Blob API Usage Pattern

The key blob API integration happens in `upload_file`:

```rust
// 1. Parse the blob ID from client-provided base58 string
let blob_id = parse_blob_id_base58(&blob_id_str)?;

// 2. Announce to network - THIS IS THE CORE BLOB API USAGE
let current_context = env::context_id();
if env::blob_announce_to_context(&blob_id, &current_context) {
    app::log!("✓ Successfully announced blob to network");
} else {
    app::log!("⚠ Warning: Failed to announce blob");
    // Still proceed - blob is stored locally
}

// 3. Store metadata for later retrieval
let file_record = FileRecord {
    blob_id,  // Store raw bytes
    // ... other fields
};
```

## Helper Functions

### Base58 Encoding/Decoding

```rust
// Encode blob ID to string
fn encode_blob_id_base58(blob_id_bytes: &[u8; 32]) -> String

// Decode string to blob ID
fn parse_blob_id_base58(blob_id_str: &str) -> Result<[u8; 32], String>

// Custom serializer for JSON
fn serialize_blob_id_bytes<S>(blob_id_bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
```

## Complete End-to-End Workflow

This section shows how the client-side blob API and contract methods work together.

### Upload Flow (Client → Contract → Network)

```typescript
// 1. CLIENT: Upload file binary to blob storage
const blobResponse = await blobClient.uploadBlob(
  file,
  onBlobProgress, // Optional progress callback
  "" // Optional expected hash
);

const blobId = blobResponse.data.blobId; // e.g., "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"

// 2. CLIENT: Call contract method with blob ID and metadata
const response = await contractApi.upload_file(
  contextId,
  file.name, // "document.pdf"
  blobId, // The blob ID from step 1
  file.size, // File size in bytes
  file.type // MIME type, e.g., "application/pdf"
);

// 3. CONTRACT: Announces blob to network (happens in upload_file method)
//    - Parses blob ID from base58 string
//    - Calls env::blob_announce_to_context(blob_id, context_id)
//    - Stores metadata in contract state
//    - Emits FileUploaded event

// 4. NETWORK: Blob is now discoverable by all nodes in the context
//    - Other nodes can request this blob
//    - Distributed storage network is established
```

### Download Flow (Client → Contract → Network → Client)

```typescript
// 1. CLIENT: Get blob ID from contract using file ID
const fileId = "file_0"; // File ID returned from upload

// Option A: Get just the blob ID
const blobId = await contractApi.get_blob_id_b58(fileId);

// Option B: Get full file metadata (includes blob ID)
const fileRecord = await contractApi.get_file(fileId);
const blobId = fileRecord.blob_id;

// 2. CLIENT: Download blob from network using blob ID
const blobData = await blobClient.downloadBlob(
  blobId, // Base58-encoded blob ID
  contextId // Context ID for network routing
);

// 3. NETWORK: Routes request to nodes that have the blob
//    - Discovers peers with this blob (from announcement)
//    - Requests blob chunks from available peers
//    - Reconstructs complete blob

// 4. CLIENT: Receives blob data as Blob object
//    - Can create download link, display, etc.
const url = URL.createObjectURL(blobData);
```

### Client-Side Integration Example

Here's how the blob API integrates with a document upload feature:

```typescript
async uploadDocument(
  contextId: string,
  name: string,
  file: File,
  onBlobProgress?: (progress: number) => void,
  onStorageProgress?: () => void,
): Promise<{ data?: string; error?: any }> {
  try {
    // Step 1: Upload blob to storage
    const blobResponse = await blobClient.uploadBlob(
      file,
      onBlobProgress,
      ''
    );

    if (blobResponse.error || !blobResponse.data?.blobId) {
      return { error: blobResponse.error };
    }

    // Step 2: Store metadata in contract
    onStorageProgress?.();

    const response = await contractApi.upload_file(
      name,
      blobResponse.data.blobId,  // Blob ID from step 1
      file.size,
      file.type
    );

    return {
      data: response.data,  // File ID from contract
      error: response.error,
    };
  } catch (error) {
    return { error: { message: `Upload error: ${error}` } };
  }
}
```

### Available Blob Client Methods

The `@calimero-network/calimero-client` provides these blob operations:

```typescript
interface BlobApi {
  // Upload a file and get its blob ID
  uploadBlob(
    file: File,
    onProgress?: (progress: number) => void,
    expectedHash?: string
  ): Promise<ApiResponse<BlobUploadResponse>>;

  // Download a blob by its ID from the network
  downloadBlob(blobId: string, contextId: string): Promise<Blob>;

  // Get metadata about a blob
  getBlobMetadata(blobId: string): Promise<ApiResponse<BlobMetadataResponse>>;

  // List all blobs
  listBlobs(): Promise<ApiResponse<BlobListResponseData>>;

  // Delete a blob
  deleteBlob(blobId: string): Promise<ApiResponse<void>>;
}
```

### Key Integration Points

1. **Blob Storage is Separate from Contract State**

   - Blob client handles binary data storage
   - Contract stores metadata (name, size, type, etc.)
   - Blob ID links them together

2. **Network Announcement is Critical**

   - `env::blob_announce_to_context()` makes blob discoverable
   - Without announcement, only the uploader can access the blob
   - Announcement enables peer-to-peer sharing

3. **Base58 Encoding for Serialization**

   - Blob IDs are 32 bytes internally
   - Converted to base58 strings for JSON/API
   - Client sends base58, contract converts to bytes

4. **Context-Based Access Control**
   - Blobs are announced to specific contexts
   - Only nodes in the same context can discover/download
   - Provides natural privacy boundaries

## Building

```bash
./build.sh
```

This will compile the contract to WebAssembly.

## Workflow Testing

The `workflows/blobs-example.yml` file provides end-to-end testing that demonstrates:

### 1. Blob API Integration

- ✓ Upload files with blob announcement (`env::blob_announce_to_context`)
- ✓ Blobs become discoverable across network nodes
- ✓ Parse and encode base58 blob IDs
- ✓ Retrieve blob IDs for downloads

### 2. Multi-Node Verification

- ✓ Files uploaded on Node 1 are visible on Node 2
- ✓ Blob announcement enables distributed access
- ✓ Deletions propagate across nodes

### 3. File Operations

- ✓ Upload multiple file types (PDF, image, text)
- ✓ List all files
- ✓ Get specific file metadata
- ✓ Search files by name
- ✓ Delete files

### 4. Storage Management

- ✓ Track total file sizes
- ✓ Get file statistics
- ✓ Monitor file counts

### 5. Error Handling

- ✓ Handle missing files gracefully
- ✓ Validate blob IDs
- ✓ Return appropriate error messages

### Blob API Workflow Diagram

**Upload Flow:**

```
Client → blobClient.uploadBlob(file) → Blob Storage
Blob Storage → returns blob_id → Client
Client → contract.upload_file(blob_id, metadata) → Contract
Contract → env::blob_announce_to_context(blob_id) → Network
Network → All nodes discover blob → Distributed Storage
```

**Download Flow:**

```
Client → contract.get_blob_id_b58(file_id) → Contract
Contract → returns blob_id → Client
Client → blobClient.downloadBlob(blob_id, context_id) → Network
Network → Finds peers with blob → Client receives data
```

## Key Takeaways

1. **Blob IDs are 32 bytes**: Always handle as `[u8; 32]` internally
2. **Use Base58 for serialization**: Convert to/from strings for JSON
3. **Announce blobs to network**: Call `env::blob_announce_to_context()` after upload
4. **Store metadata separately**: Blobs are content-addressed; metadata is in contract state
5. **Events for UI updates**: Emit events for real-time client synchronization
