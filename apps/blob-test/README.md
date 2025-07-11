# Chat Application with Blob Attachments

A demonstration chat application that showcases the blob API functionality with compressed file attachments.

## Features

- **Real-time chat interface**: Send messages with text and file attachments
- **Immediate file upload**: Files are uploaded via HTTP REST API as soon as they're selected
- **Automatic compression**: Attachments are compressed and stored using the blob streaming API
- **Message history**: View all messages with expandable attachment details
- **File download**: Download and decompress attachments from messages
- **Compression statistics**: View storage savings from compression

## Architecture

### Frontend (React)
- **Chat interface**: Modern chat UI with drag-and-drop file support
- **File upload**: Immediate upload via HTTP REST API with progress tracking
- **Message management**: Send, view, and interact with messages
- **Attachment handling**: Download and view compressed attachments

### Backend (Rust)
- **Message storage**: Store messages with text and attachment metadata
- **Blob compression**: Read uploaded blobs, compress them using RLE, and store compressed versions
- **Streaming API**: Use `blob_create()`, `blob_write()`, `blob_close()` for writing; `blob_open()`, `blob_read()`, `blob_close()` for reading
- **Decompression**: Restore original files when downloading attachments

## How It Works

1. **File Upload Flow**:
   - User selects files (drag-and-drop or file picker)
   - Files are immediately uploaded via HTTP API to get blob IDs
   - Upload progress is tracked and displayed

2. **Message Sending Flow**:
   - User types message and hits send
   - Backend reads each attachment blob using streaming API
   - Files are compressed using RLE (Run-Length Encoding)
   - Compressed data is stored as new blobs using streaming API
   - Message is stored with text and compressed attachment metadata

3. **File Download Flow**:
   - User clicks download on an attachment
   - Backend reads compressed blob using streaming API
   - Data is decompressed and returned to frontend
   - Browser downloads the restored original file

## Blob API Usage

### Writing Blobs (Compression)
```rust
let fd = env::blob_create();
let bytes_written = env::blob_write(fd, data);
let blob_id = env::blob_close(fd);
```

### Reading Blobs (Decompression)
```rust
let fd = env::blob_open(blob_id.as_ref());
let bytes_read = env::blob_read(fd, &mut buffer);
let _cleanup = env::blob_close(fd);
```

## Compression

The application uses a simple Run-Length Encoding (RLE) compression algorithm for demonstration. In production, you would use more sophisticated compression like gzip, brotli, or lz4.

## Running the Application

1. Build the Rust application:
   ```bash
   ./build.sh
   ```

2. Start the frontend:
   ```bash
   cd frontend
   npm install
   npm start
   ```

3. Open http://localhost:3000 in your browser

## API Methods

- `send_message`: Send a message with text and attachment blob IDs
- `get_messages`: Retrieve all messages
- `get_message`: Get a specific message by ID
- `get_attachment_data`: Download and decompress an attachment
- `get_stats`: Get compression and usage statistics
- `clear_messages`: Clear all messages (for testing)

## Statistics

The application tracks:
- Total messages sent
- Total attachments processed
- Original file sizes vs compressed sizes
- Compression savings percentage 