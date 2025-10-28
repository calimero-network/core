# Collaborative Editor

A real-time collaborative text editor built on Calimero's RGA (Replicated Growable Array) CRDT.

## Overview

This application demonstrates conflict-free collaborative editing where multiple users can edit the same document simultaneously without manual conflict resolution. The RGA CRDT ensures that all nodes eventually converge to the same document state, even when edits are made concurrently.

## Features

- **Real-time Collaboration**: Multiple users can edit the same document simultaneously
- **Conflict-Free**: RGA CRDT automatically resolves conflicts deterministically
- **Character-level Operations**: Insert and delete text at any position
- **Distributed Edit Counting**: Uses Counter CRDT to track total edits across all nodes
- **Document Metadata**: Title management and statistics
- **Event Emission**: Tracks all document changes via events

## CRDT Types Used

1. **ReplicatedGrowableArray (RGA)**: The main document storage that handles collaborative text editing
2. **Counter**: Tracks the total number of edits made across all nodes

## Building

```bash
./build.sh
```

This will:
1. Build the WASM contract
2. Generate the ABI manifest
3. Optimize the WASM binary (if `wasm-opt` is available)
4. Output to `res/collaborative_editor.wasm`

## API Methods

### Initialization

#### `init(title: String) -> EditorState`
Initialize a new collaborative document.

**Parameters:**
- `title`: The document title/name

**Example:**
```json
{
  "title": "My Shared Document"
}
```

---

### Text Operations

#### `insert_text(position: usize, text: String) -> Result<(), String>`
Insert text at a specific position.

**Parameters:**
- `position`: The position to insert text (0-indexed)
- `text`: The text to insert

**Example:**
```json
{
  "position": 0,
  "text": "Hello"
}
```

#### `delete_text(start: usize, end: usize) -> Result<(), String>`
Delete text in a range.

**Parameters:**
- `start`: Starting position (inclusive, 0-indexed)
- `end`: Ending position (exclusive, 0-indexed)

**Example:**
```json
{
  "start": 0,
  "end": 5
}
```

#### `replace_text(start: usize, end: usize, text: String) -> Result<(), String>`
Replace a range of text with new text (atomic operation).

**Parameters:**
- `start`: Starting position (inclusive, 0-indexed)
- `end`: Ending position (exclusive, 0-indexed)
- `text`: The new text to insert

**Example:**
```json
{
  "start": 0,
  "end": 5,
  "text": "Goodbye"
}
```

#### `append_text(text: String) -> Result<(), String>`
Append text to the end of the document.

**Parameters:**
- `text`: The text to append

**Example:**
```json
{
  "text": " World!"
}
```

#### `clear() -> Result<(), String>`
Clear the entire document.

---

### Query Methods

#### `get_text() -> Result<String, String>`
Get the current document text.

**Returns:** The complete document as a string

**Example Response:**
```json
{
  "output": "Hello World!"
}
```

#### `get_length() -> Result<usize, String>`
Get the length of the document in characters.

**Returns:** Number of characters

**Example Response:**
```json
{
  "output": 12
}
```

#### `is_empty() -> Result<bool, String>`
Check if the document is empty.

**Returns:** True if empty, false otherwise

**Example Response:**
```json
{
  "output": false
}
```

---

### Title Management

#### `set_title(new_title: String) -> Result<(), String>`
Set the document title.

**Parameters:**
- `new_title`: The new document title (cannot be empty)

**Example:**
```json
{
  "new_title": "Updated Document Title"
}
```

#### `get_title() -> String`
Get the current document title.

**Returns:** The document title

**Example Response:**
```json
{
  "output": "My Shared Document"
}
```

---

### Statistics

#### `get_stats() -> Result<String, String>`
Get document statistics including title, length, total edits, and owner.

**Returns:** Formatted statistics string

**Example Response:**
```json
{
  "output": "Document Statistics:\n- Title: My Shared Document\n- Length: 42 characters\n- Total edits: 15\n- Owner: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
}
```

---

## Events

### `DocumentCreated`
Emitted when a new document is initialized.

**Fields:**
- `title`: Document title
- `owner`: Owner's identity

### `TextInserted`
Emitted when text is inserted.

**Fields:**
- `position`: Position where text was inserted
- `text`: The text that was inserted
- `editor`: Editor's identity who made the change

### `TextDeleted`
Emitted when text is deleted.

**Fields:**
- `start`: Starting position of deletion
- `end`: Ending position of deletion
- `editor`: Editor's identity who made the change

### `TitleChanged`
Emitted when the document title is changed.

**Fields:**
- `old_title`: Previous title
- `new_title`: New title
- `editor`: Editor's identity who made the change

---

## Testing

Run the end-to-end workflow test:

```bash
# From the e2e-tests directory
cargo run -- workflows/collaborative-editor.yml
```

The workflow tests:
- Basic insert/append operations
- Delete operations
- Text replacement
- Concurrent edits from multiple nodes
- CRDT convergence properties
- Title management
- Document statistics
- Error handling (invalid positions, ranges, etc.)

## How RGA Works

The RGA (Replicated Growable Array) CRDT ensures conflict-free collaborative editing:

1. **Character Identity**: Each character has a unique ID combining HLC timestamp and sequence number
2. **Ordering Metadata**: Each character stores a reference to its left neighbor
3. **Deterministic Resolution**: When concurrent edits occur, RGA uses character IDs to determine order
4. **Tombstone Deletion**: Deleted characters are marked but preserved for ordering
5. **Convergence**: All nodes eventually converge to the same document state

### Example: Concurrent Edits

```
Initial: ""

Node 1: insert(0, "ABC")  ->  "ABC"
Node 2: insert(0, "XYZ")  ->  "XYZ"

After sync, both nodes converge to the same result (e.g., "ABCXYZ" or "XYZABC")
based on deterministic HLC timestamp ordering.
```

## Architecture

```
EditorState
├── document: ReplicatedGrowableArray  (RGA CRDT for text)
├── title: String                       (Document title)
├── owner: String                       (Owner identity)
└── edit_count: Counter                 (CRDT counter for edits)
```

## Use Cases

- Collaborative document editing
- Shared notes and wikis
- Real-time code editing
- Chat message editing
- Any scenario requiring conflict-free text collaboration

## Limitations

- Character-level operations (not optimized for very large documents)
- No rich text formatting (plain text only)
- Tombstones are never garbage collected (see storage crate for GC)

## Related Documentation

- [RGA CRDT Implementation](../../crates/storage/src/collections/rga.rs)
- [Counter CRDT](../../crates/storage/src/collections/counter.rs)
- [Calimero Storage](../../crates/storage/README.md)
- [Calimero SDK](../../crates/sdk/README.md)

