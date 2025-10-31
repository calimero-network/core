# merodb

A CLI tool for debugging RocksDB databases used by Calimero.

## Features

- **Schema Generation**: Generate a JSON schema describing the structure of all column families.
- **Data Export**: Export database contents to pretty-printed JSON with ABI-aware decoding of contract state.
- **Data Validation**: Validate database integrity and detect corruption.
- **Interactive GUI**: Web-based interface for browsing exported data and running JQ queries (optional feature).

## Installation

Build the CLI tool:

```bash
cargo build --package merodb --release
```

The binary will be available at `target/release/merodb`.

### Building with GUI Support

To enable the interactive web GUI feature:

```bash
cargo build --package merodb --release --features gui
```

## Usage

Most commands operate on a RocksDB data directory. When exporting contract state or Context DAG deltas, you must also supply the compiled WASM artifact that embeds the ABI manifest (`--wasm-file /path/to/contract.wasm`). Without the manifest the tool falls back to hex dumps for state values.

### Generate Database Schema

Generate a JSON schema describing all column families and their structures:

```bash
merodb --db-path /path/to/rocksdb --schema
```

Save to a file:

```bash
merodb --db-path /path/to/rocksdb --schema --output schema.json
```

### Export Data

Export all data from the database:

```bash
merodb --db-path /path/to/rocksdb --export --all --output export.json --wasm-file /path/to/contract.wasm
```

Export specific column families:

```bash
merodb --db-path /path/to/rocksdb --export --columns Meta,Config,State --wasm-file /path/to/contract.wasm
```

### Validate Database

Validate the database integrity:

```bash
merodb --db-path /path/to/rocksdb --validate --output validation.json
```

### Interactive GUI (requires `gui` feature)

Launch the web-based GUI to interactively explore your database:

```bash
merodb --gui
```

The GUI will start a local web server (default port 8080). You can then:

1. Enter your RocksDB database folder path
2. Upload your instrumented WASM contract file
3. Click "Load Database" to process and view the data
4. Browse the database structure with an interactive tree view
5. Run JQ queries to filter and analyze the data
6. Explore query results in real-time

Specify a custom port:

```bash
merodb --gui --port 3000
```

**Workflow with GUI:**

```bash
# 1. Launch the GUI
merodb --gui

# 2. Open http://127.0.0.1:8080 in your browser
# 3. Enter database path: ~/.calimero/data
# 4. Upload WASM file: contract.wasm
# 5. Click "Load Database" and start exploring with JQ queries
```

The GUI automatically exports and processes the database server-side, eliminating the need for manual JSON export.

**Example JQ Queries in GUI:**

- `.data` - View all column families
- `.data | keys` - List all column family names
- `.data.Meta.entries[0]` - View first Meta entry
- `.data.State.entries | map(.key)` - Extract all state keys
- `.data | to_entries | map({column: .key, count: .value.count})` - Get entry counts per column

## Column Families

The tool supports all Calimero RocksDB column families:

- **Meta**: Context metadata (application ID, root hash)
- **Config**: Context configuration (protocol, network, contracts, revisions)
- **Identity**: Context membership (private key and sender key pairs)
- **State**: Application-specific state values decoded through the contract ABI
- **Blobs**: Blob metadata (size, hash, links to other blobs)
- **Application**: Application metadata (bytecode, size, source, metadata, compiled blob)
- **Alias**: Human-readable aliases for contexts, applications, and public keys
- **Generic**: Generic key-value storage (Context DAG deltas and arbitrary values)

## Data Formats

### Automatic Type Detection

The tool automatically detects and decodes known Calimero types using Borsh deserialization:

- **ContextMeta**: `{ application: ApplicationId, root_hash: Hash }`
- **ContextConfig**: `{ protocol, network, contract, proxy_contract, application_revision, members_revision }`
- **ContextIdentity**: `{ private_key: Option<[u8; 32]>, sender_key: Option<[u8; 32]> }`
- **BlobMeta**: `{ size: u64, hash: [u8; 32], links: Box<[BlobId]> }`
- **ApplicationMeta**: `{ bytecode: BlobId, size: u64, source: Box<str>, metadata: Box<[u8]>, compiled: BlobId }`
- **ContextDagDelta**: `{ delta_id, parents, actions, timestamp, hlc, applied }` with detailed HLC breakdown

### Unknown Data

When no ABI is supplied (or the ABI lacks a matching record) the tool exports raw hexadecimal strings and records a note explaining the fallback.

### Implementation Note

The tool imports types directly from the `calimero-store` crate to ensure parsing matches the exact structure used by Calimero. The contract ABI is emitted by `calimero-wasm-abi` at build time and is required to resolve application-specific state keys and values.

## Read-Only Access

The tool opens the database in **read-only mode**, which means:

- Safe to use while the Calimero node is running
- No risk of corrupting the database
- Multiple instances can read simultaneously

## Examples

### Complete Database Inspection Workflow

```bash
# 1. Generate schema to understand structure
merodb --schema --output schema.json

# 2. Validate database integrity
merodb --db-path ~/.calimero/data --validate --output validation.json

# 3. Export all data for analysis
merodb --db-path ~/.calimero/data --export --all --output full-export.json

# 4. Export only context-related data
merodb --db-path ~/.calimero/data --export --columns Meta,Config,Identity --output contexts.json
```

### Debugging a Specific Context

```bash
# Export context metadata and configuration
merodb --db-path ~/.calimero/data --export --columns Meta,Config --output context-info.json

# Analyze the JSON to find your context ID
# Then export state and deltas for that context
merodb --db-path ~/.calimero/data --export --columns State,Generic --output context-state.json --wasm-file ./calimero_marketplace.wasm
```

## Output Format

All commands output pretty-printed JSON. Example schema output:

```json
{
  "database": "Calimero RocksDB",
  "version": "1.0",
  "description": "Schema for Calimero's RocksDB column families",
  "columns": {
    "Meta": {
      "name": "Meta",
      "key": {
        "structure": "ContextId (32 bytes)",
        "size_bytes": 32
      },
      "value": {
        "structure": "ContextMeta { application: ApplicationId, root_hash: Hash }",
        "description": "Stores context metadata..."
      }
    }
  }
}
```
