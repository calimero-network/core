# File Sharing App - Blob API Implementation

A minimal demonstration of the Calimero blob API for building decentralized file sharing applications.

## Overview

This application demonstrates how to use the Calimero blob storage API to build a simple file sharing backend. It shows the core patterns for:

- **Blob Storage**: Storing files as blobs with metadata
- **Network Announcement**: Making blobs discoverable across the network
- **File Management**: Upload, delete, list, and search files
- **Hex Encoding**: Safe serialization of binary blob IDs

## Key Concepts

### Blob IDs

Blobs are identified by 32-byte IDs, modelled by the SDK's `BlobId` newtype
(`calimero_sdk::BlobId`):

- **In Rust**: `BlobId` - a 32-byte ID with `Display`/`FromStr` and serde/borsh impls
- **Over the wire**: a 64-character hex string - `BlobId` (de)serializes to/from hex in JSON

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
    pub id: String,              // Unique file ID: "<uploader account hex>_<nonce hex>"
    pub name: String,            // Human-readable name
    pub blob_id: BlobId,         // Blob ID (hex string in JSON via the SDK newtype)
    pub size: u64,               // File size in bytes
    pub mime_type: String,       // Content type
    pub uploaded_by: String,     // Uploader's account, read from the owner stamp
    pub uploaded_at: u64,        // Timestamp
}
```

### FileShareState

Application state using Calimero storage collections:

```rust
pub struct FileShareState {
    pub owner: Frozen<String>,                    // The creating account, set once in init
    pub files: AuthoredMap<String, FileRecord>,   // ID -> FileRecord, each its uploader's own
}
```

## API Methods

### Upload a File

```rust
upload_file(
    name: String,
    blob_id: BlobId,  // Blob ID (hex string over the wire)
    size: u64,
    mime_type: String
) -> app::Result<String>
```

**Process:**

1. Generate a unique file ID naming the uploader
2. **Announce blob to network** (key blob API usage)
3. Store file metadata
4. Emit event

**Example:**

```rust
let file_id = state.upload_file(
    "document.pdf".to_string(),
    "a3f1c0de9b7e44d2a8f5061c3b2e9d7f5a4c1b0e8d7f6a5b4c3d2e1f0a9b8c7d".parse()?,
    1024000,
    "application/pdf".to_string()
)?;
```

### Delete a File

```rust
delete_file(file_id: String) -> app::Result<()>
```

Removes the caller's own file record from storage and emits a deletion event. Only the uploader may delete a record; see "Who may change a file record".

### List All Files

```rust
list_files() -> app::Result<Vec<FileRecord>>
```

Returns all stored files with their metadata.

### Get Specific File

```rust
get_file(file_id: String) -> app::Result<FileRecord>
```

Retrieves a single file's metadata by ID.

### Get Blob ID

```rust
get_blob_id_hex(file_id: String) -> app::Result<BlobId>
```

Returns the blob ID for a file, a hex string over the wire (useful for downloading).

### Search Files

```rust
search_files(query: String) -> app::Result<Vec<FileRecord>>
```

Case-insensitive search by filename.

### Statistics

```rust
get_stats() -> app::Result<String>
get_total_files_size() -> app::Result<u64>
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
// 1. `blob_id: BlobId` is already parsed from its hex string by the SDK.

// 2. Announce to network - THIS IS THE CORE BLOB API USAGE
let current_context = env::context_id();
if env::blob_announce_to_context(blob_id.as_ref(), &current_context) {
    app::log!("✓ Successfully announced blob to network");
} else {
    app::log!("⚠ Warning: Failed to announce blob");
    // Still proceed - blob is stored locally
}

// 3. Store metadata for later retrieval
let file_record = FileRecord {
    blob_id,  // Store the BlobId
    // ... other fields
};
```

## Blob ID Encoding

Hex ↔ bytes conversion is owned by the SDK's `BlobId` newtype
(`calimero_sdk::BlobId`), so this app no longer hand-rolls encode/decode
helpers:

```rust
use calimero_sdk::BlobId;

// Hex string -> BlobId (FromStr)
let blob_id: BlobId = "a3f1c0de9b7e44d2a8f5061c3b2e9d7f5a4c1b0e8d7f6a5b4c3d2e1f0a9b8c7d".parse()?;

// BlobId -> hex string (Display)
let encoded = blob_id.to_string();

// BlobId -> &[u8; 32] (for env/blob host functions)
let bytes: &[u8; 32] = blob_id.as_ref();
```

## Complete End-to-End Workflow

This section shows how the client-side blob API and contract methods work together.

### Upload Flow (Client → Contract → Network)

```typescript
// 1. CLIENT: Upload file binary to blob storage, for the context (mero-js).
//    The context id is what makes the blob the context's, so the announce
//    in step 3 can share it.
const { blobId } = await mero.admin.uploadBlob({ data: file, contextId });
// e.g., "a3f1c0de9b7e44d2a8f5061c3b2e9d7f5a4c1b0e8d7f6a5b4c3d2e1f0a9b8c7d"

// 2. CLIENT: Call contract method with blob ID and metadata
const response = await contractApi.upload_file(
  contextId,
  file.name, // "document.pdf"
  blobId, // The blob ID from step 1
  file.size, // File size in bytes
  file.type // MIME type, e.g., "application/pdf"
);

// 3. CONTRACT: Announces blob to network (happens in upload_file method)
//    - Parses blob ID from its hex string
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
const fileId = uploadedFileId; // File ID returned from upload ("<uploader account hex>_<nonce>")

// Option A: Get just the blob ID
const blobId = await contractApi.get_blob_id_hex(fileId);

// Option B: Get full file metadata (includes blob ID)
const fileRecord = await contractApi.get_file(fileId);
const blobId = fileRecord.blob_id;

// 2. CLIENT: Download blob from network using blob ID
const blobData = await blobClient.downloadBlob(
  blobId, // Hex-encoded blob ID
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
  onStorageProgress?: () => void,
): Promise<{ data?: string; error?: any }> {
  try {
    // Step 1: Upload blob to storage, for the context (mero-js)
    const { blobId } = await mero.admin.uploadBlob({ data: file, contextId });

    // Step 2: Store metadata in contract
    onStorageProgress?.();

    const response = await contractApi.upload_file(
      name,
      blobId,  // Blob ID from step 1
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

The `@calimero-network/calimero-client` provides these blob operations. Its
`uploadBlob` cannot name a context, so its uploads are held for none and the
app's announce shares nothing; upload through mero-js with a `contextId`.

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
   - Only a blob held for the context is announced: upload it with the context's id
   - A blob uploaded without a context is held for none, and announcing it shares nothing

3. **Hex Encoding for Serialization**

   - Blob IDs are 32 bytes internally
   - Converted to 64-character hex strings for JSON/API
   - Client sends hex, the SDK's `BlobId` converts it to bytes

4. **Context-Based Access Control**
   - Blobs are announced to specific contexts
   - Only nodes in the same context can discover/download
   - Provides natural privacy boundaries

## Who may change a file record

Each record is its uploader's own entry in an `AuthoredMap`, so the rules
below hold on every node, against a patched one too:

- **Only the uploader changes or deletes a record.** `delete_file` checks it
  first for a readable error; the storage layer refuses anyone else's removal
  when it applies the write.
- **The uploader shown is the owner stamp.** `uploaded_by` is filled in from
  the entry's owner on every read, so whatever a writer stored there is never
  shown.
- **A file id names its uploader.** `upload_file` mints
  `"<uploader account hex>_<nonce hex>"`. Keys are per owner, so any account
  could hold an entry at the same id; every read by id reads only the entry of
  the account the id names (`get_by`), and `list_files`, `search_files` and the
  totals drop any record held by another account. The nonce, not a shared
  counter, keeps two devices of one account from minting the same id at once.
- **The context owner is `Frozen`**: written once in `init` from the creating
  account, and no node accepts a change.

## Building

```bash
cargo mero build
```

This will compile the contract to WebAssembly.

## Workflow Testing

The `workflows/blobs-example.yml` file provides end-to-end testing that demonstrates:

### 1. Blob API Integration

- ✓ Upload files with blob announcement (`env::blob_announce_to_context`)
- ✓ Blobs become discoverable across network nodes
- ✓ Parse and encode hex blob IDs
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
Client → blobClient.uploadBlob(file, context_id) → Blob Storage
Blob Storage → returns blob_id → Client
Client → contract.upload_file(blob_id, metadata) → Contract
Contract → env::blob_announce_to_context(blob_id) → Network
Network → All nodes discover blob → Distributed Storage
```

**Download Flow:**

```
Client → contract.get_blob_id_hex(file_id) → Contract
Contract → returns blob_id → Client
Client → blobClient.downloadBlob(blob_id, context_id) → Network
Network → Finds peers with blob → Client receives data
```

## Key Takeaways

1. **Blob IDs are 32 bytes**: Always handle as `[u8; 32]` internally
2. **Use hex for serialization**: `BlobId` converts to/from strings for JSON
3. **Announce blobs to network**: Upload with the context's id, then call `env::blob_announce_to_context()`
4. **Store metadata separately**: Blobs are content-addressed; metadata is in contract state
5. **Events for UI updates**: Emit events for real-time client synchronization
