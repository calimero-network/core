use serde_json::{json, Value};

use crate::types::Column;

/// Generate a JSON schema describing the RocksDB structure
pub fn generate_schema() -> Value {
    let mut columns = serde_json::Map::new();

    for column in Column::all() {
        let column_info = json!({
            "name": column.as_str(),
            "key": {
                "structure": column.key_structure(),
                "size_bytes": column.key_size()
            },
            "value": {
                "structure": column.value_structure(),
                "description": get_column_description(*column)
            }
        });

        drop(columns.insert(column.as_str().to_owned(), column_info));
    }

    json!({
        "database": "Calimero RocksDB",
        "version": "1.0",
        "description": "Schema for Calimero's RocksDB column families",
        "columns": columns
    })
}

const fn get_column_description(column: Column) -> &'static str {
    match column {
        Column::Meta => "Stores context metadata including application ID and root hash. Each context has exactly one metadata entry.",
        Column::Config => "Stores context configuration including protocol, network, contract addresses, and revision numbers.",
        Column::Identity => "Stores context membership. Each entry represents a public key that is a member of a context.",
        Column::State => "Stores application-specific state as raw bytes. The structure depends on the application.",
        Column::Delta => "Stores state changes (deltas) by identity and block height. Used for tracking state modifications.",
        Column::Blobs => "Stores blob metadata including size and content type. The actual blob data is stored separately.",
        Column::Application => "Stores application metadata including the blob ID and source hash.",
        Column::Alias => "Stores human-readable aliases for contexts, applications, and public keys.",
        Column::Generic => "Generic key-value storage for arbitrary data organized by scope and fragment.",
    }
}
