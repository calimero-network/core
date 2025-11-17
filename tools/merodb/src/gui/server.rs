use std::path::PathBuf;

use axum::{
    extract::Multipart,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use rocksdb::{DBWithThreadMode, Options, SingleThreaded};
use serde::Serialize;
use tower_http::services::ServeDir;

use crate::{abi, dag, export, types::Column};

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Serialize)]
struct ExportResponse {
    data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    info: Option<String>,
}

pub async fn start_gui_server(port: u16) -> eyre::Result<()> {
    // Get the directory containing the GUI files
    let gui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("gui");

    let static_dir = gui_dir.join("static");

    // Serve static files from /static/*
    let serve_static = ServeDir::new(&static_dir).append_index_html_on_directories(false);

    let app = Router::new()
        .route("/", get(render_app))
        .route("/api/export", post(handle_export))
        .route("/api/dag", post(handle_dag))
        .route("/api/dag/delta-details", post(handle_dag_delta_details))
        .route("/api/state-tree", post(handle_state_tree))
        .route("/api/contexts", post(handle_list_contexts))
        .route("/api/context-tree", post(handle_context_tree))
        .route("/api/validate-abi", post(handle_validate_abi))
        .nest_service("/static", serve_static);

    let addr = format!("127.0.0.1:{port}");
    println!("Starting GUI server at http://{addr}");
    println!("Serving static files from: {}", static_dir.display());
    println!("Press Ctrl+C to stop the server");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| eyre::eyre!("Failed to bind to {}: {}", addr, e))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| eyre::eyre!("Server error: {}", e))?;

    Ok(())
}

async fn render_app() -> Html<String> {
    Html(include_str!("index.html").to_owned())
}

async fn handle_export(mut multipart: Multipart) -> impl IntoResponse {
    let mut db_path: Option<PathBuf> = None;
    let mut wasm_bytes: Option<Vec<u8>> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();

        match name.as_str() {
            "db_path" => {
                if let Ok(value) = field.text().await {
                    db_path = Some(PathBuf::from(value));
                }
            }
            "wasm_file" => {
                if let Ok(bytes) = field.bytes().await {
                    wasm_bytes = Some(bytes.to_vec());
                }
            }
            _ => {}
        }
    }

    // Validate inputs
    let Some(db_path) = db_path else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Database path is required".to_owned(),
            }),
        )
            .into_response();
    };

    // Check if database path exists first
    if !db_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Database path does not exist: {}", db_path.display()),
            }),
        )
            .into_response();
    }

    // Validate path to prevent traversal attacks (requires path to exist)
    if let Err(e) = validate_db_path(&db_path) {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response();
    }

    // Extract ABI from WASM bytes (if provided)
    let mut warning_message = None;
    let mut info_message = None;
    let abi_manifest = if let Some(wasm_bytes) = wasm_bytes {
        match abi::extract_abi_from_wasm_bytes(&wasm_bytes) {
            Ok(manifest) => {
                info_message = Some(
                    "Successfully extracted ABI from WASM file. State values will be decoded using the ABI schema.".to_string()
                );
                Some(manifest)
            }
            Err(e) => {
                let warning = format!("The uploaded WASM file does not contain an exported ABI. The file may not have been built with ABI support. State values will not be decoded. Error: {e}");
                eprintln!("Warning: {warning}");
                warning_message = Some(warning);
                None
            }
        }
    } else {
        eprintln!("No WASM file provided - state values will not be decoded");
        None
    };

    // Open database
    let db = match open_database(&db_path) {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to open database: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Export all columns
    let columns = Column::all().to_vec();
    let data = if let Some(manifest) = abi_manifest {
        match export::export_data(&db, &columns, &manifest) {
            Ok(data) => data,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to export data: {e}"),
                    }),
                )
                    .into_response();
            }
        }
    } else {
        // Export without ABI - use a dummy manifest
        match export::export_data_without_abi(&db, &columns) {
            Ok(data) => data,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to export data: {e}"),
                    }),
                )
                    .into_response();
            }
        }
    };

    (
        StatusCode::OK,
        Json(ExportResponse {
            data,
            warning: warning_message,
            info: info_message,
        }),
    )
        .into_response()
}

/// Legacy endpoint - returns all contexts with full trees (slow for multi-context DBs)
/// Use /api/contexts and /api/context-tree instead for better performance
async fn handle_state_tree(mut multipart: Multipart) -> impl IntoResponse {
    let mut db_path: Option<PathBuf> = None;
    let mut wasm_bytes: Option<Vec<u8>> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();

        match name.as_str() {
            "db_path" => {
                if let Ok(value) = field.text().await {
                    db_path = Some(PathBuf::from(value));
                }
            }
            "wasm_file" => {
                if let Ok(bytes) = field.bytes().await {
                    wasm_bytes = Some(bytes.to_vec());
                }
            }
            _ => {}
        }
    }

    // Validate inputs
    let Some(db_path) = db_path else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Database path is required".to_owned(),
            }),
        )
            .into_response();
    };

    // Check if database path exists first
    if !db_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Database path does not exist: {}", db_path.display()),
            }),
        )
            .into_response();
    }

    // Validate path to prevent traversal attacks (requires path to exist)
    if let Err(e) = validate_db_path(&db_path) {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response();
    }

    // WASM is required for state tree extraction
    let Some(wasm_bytes) = wasm_bytes else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "WASM file is required for state tree extraction".to_owned(),
            }),
        )
            .into_response();
    };

    // Extract ABI from WASM bytes
    let abi_manifest = match abi::extract_abi_from_wasm_bytes(&wasm_bytes) {
        Ok(manifest) => manifest,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Failed to extract ABI from WASM: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Open database
    let db = match open_database(&db_path) {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to open database: {e}"),
                }),
            )
                .into_response();
        }
    };

    // List contexts first
    let contexts = match export::list_contexts(&db) {
        Ok(contexts) => contexts,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to list contexts: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Build trees for all contexts (legacy behavior - not recommended for many contexts)
    let mut context_trees = Vec::new();
    for context_info in contexts {
        let context_id = match context_info.get("context_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };

        match export::extract_context_tree(&db, context_id, &abi_manifest) {
            Ok(tree) => context_trees.push(tree),
            Err(e) => {
                eprintln!("Warning: Failed to build tree for context {context_id}: {e}");
            }
        }
    }

    (
        StatusCode::OK,
        Json(ExportResponse {
            data: serde_json::json!({
                "contexts": context_trees,
                "total_contexts": context_trees.len()
            }),
            warning: Some(
                "This endpoint loads all contexts at once. For better performance with many contexts, use /api/contexts and /api/context-tree instead.".to_string()
            ),
            info: None,
        }),
    )
        .into_response()
}

/// List all available contexts without building trees (fast operation)
async fn handle_list_contexts(mut multipart: Multipart) -> impl IntoResponse {
    let mut db_path: Option<PathBuf> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();
        if name.as_str() == "db_path" {
            if let Ok(value) = field.text().await {
                db_path = Some(PathBuf::from(value));
            }
        }
    }

    // Validate inputs
    let Some(db_path) = db_path else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Database path is required".to_owned(),
            }),
        )
            .into_response();
    };

    // Check if database path exists first
    if !db_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Database path does not exist: {}", db_path.display()),
            }),
        )
            .into_response();
    }

    // Validate path to prevent traversal attacks
    if let Err(e) = validate_db_path(&db_path) {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response();
    }

    // Open database
    let db = match open_database(&db_path) {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to open database: {e}"),
                }),
            )
                .into_response();
        }
    };

    // List contexts
    let contexts = match export::list_contexts(&db) {
        Ok(contexts) => contexts,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to list contexts: {e}"),
                }),
            )
                .into_response();
        }
    };

    (
        StatusCode::OK,
        Json(ExportResponse {
            data: serde_json::json!({
                "contexts": contexts,
                "total_contexts": contexts.len()
            }),
            warning: None,
            info: None,
        }),
    )
        .into_response()
}

/// Extract state tree for a specific context
async fn handle_context_tree(mut multipart: Multipart) -> impl IntoResponse {
    let mut db_path: Option<PathBuf> = None;
    let mut wasm_bytes: Option<Vec<u8>> = None;
    let mut context_id: Option<String> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();

        match name.as_str() {
            "db_path" => {
                if let Ok(value) = field.text().await {
                    db_path = Some(PathBuf::from(value));
                }
            }
            "wasm_file" => {
                if let Ok(bytes) = field.bytes().await {
                    wasm_bytes = Some(bytes.to_vec());
                }
            }
            "context_id" => {
                if let Ok(value) = field.text().await {
                    context_id = Some(value);
                }
            }
            _ => {}
        }
    }

    // Validate inputs
    let Some(db_path) = db_path else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Database path is required".to_owned(),
            }),
        )
            .into_response();
    };

    let Some(context_id) = context_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Context ID is required".to_owned(),
            }),
        )
            .into_response();
    };

    // Check if database path exists first
    if !db_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Database path does not exist: {}", db_path.display()),
            }),
        )
            .into_response();
    }

    // Validate path to prevent traversal attacks
    if let Err(e) = validate_db_path(&db_path) {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response();
    }

    // WASM is required for state tree extraction
    let Some(wasm_bytes) = wasm_bytes else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "WASM file is required for state tree extraction".to_owned(),
            }),
        )
            .into_response();
    };

    // Extract ABI from WASM bytes
    let abi_manifest = match abi::extract_abi_from_wasm_bytes(&wasm_bytes) {
        Ok(manifest) => manifest,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Failed to extract ABI from WASM: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Open database
    let db = match open_database(&db_path) {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to open database: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Extract tree for specific context
    let tree_data = match export::extract_context_tree(&db, &context_id, &abi_manifest) {
        Ok(data) => data,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to extract context tree: {e}"),
                }),
            )
                .into_response();
        }
    };

    (
        StatusCode::OK,
        Json(ExportResponse {
            data: tree_data,
            warning: None,
            info: None,
        }),
    )
        .into_response()
}

async fn handle_dag(mut multipart: Multipart) -> impl IntoResponse {
    let mut db_path: Option<PathBuf> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();
        if name.as_str() == "db_path" {
            if let Ok(value) = field.text().await {
                db_path = Some(PathBuf::from(value));
            }
        }
    }

    // Validate inputs
    let Some(db_path) = db_path else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Database path is required".to_owned(),
            }),
        )
            .into_response();
    };

    // Check if database path exists first
    if !db_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Database path does not exist: {}", db_path.display()),
            }),
        )
            .into_response();
    }

    // Validate path to prevent traversal attacks (requires path to exist)
    if let Err(e) = validate_db_path(&db_path) {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response();
    }

    // Open database
    let db = match open_database(&db_path) {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to open database: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Export DAG structure
    let dag_data = match dag::export_dag(&db) {
        Ok(data) => data,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to export DAG: {e}"),
                }),
            )
                .into_response();
        }
    };

    (StatusCode::OK, Json(dag_data)).into_response()
}

/// Get detailed information about a specific delta (on-demand loading for tooltips)
async fn handle_dag_delta_details(mut multipart: Multipart) -> impl IntoResponse {
    let mut db_path: Option<PathBuf> = None;
    let mut context_id: Option<String> = None;
    let mut delta_id: Option<String> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();
        match name.as_str() {
            "db_path" => {
                if let Ok(value) = field.text().await {
                    db_path = Some(PathBuf::from(value));
                }
            }
            "context_id" => {
                if let Ok(value) = field.text().await {
                    context_id = Some(value);
                }
            }
            "delta_id" => {
                if let Ok(value) = field.text().await {
                    delta_id = Some(value);
                }
            }
            _ => {}
        }
    }

    // Validate inputs
    let Some(db_path) = db_path else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Database path is required".to_owned(),
            }),
        )
            .into_response();
    };

    let Some(context_id) = context_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Context ID is required".to_owned(),
            }),
        )
            .into_response();
    };

    let Some(delta_id) = delta_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Delta ID is required".to_owned(),
            }),
        )
            .into_response();
    };

    // Check if database path exists
    if !db_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Database path does not exist: {}", db_path.display()),
            }),
        )
            .into_response();
    }

    // Validate path to prevent traversal attacks
    if let Err(e) = validate_db_path(&db_path) {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response();
    }

    // Open database
    let db = match open_database(&db_path) {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to open database: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Get delta details
    let details = match dag::get_delta_details(&db, &context_id, &delta_id) {
        Ok(data) => data,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get delta details: {e}"),
                }),
            )
                .into_response();
        }
    };

    (StatusCode::OK, Json(details)).into_response()
}

async fn handle_validate_abi(mut multipart: Multipart) -> impl IntoResponse {
    let mut wasm_bytes: Option<Vec<u8>> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();
        if name.as_str() == "wasm_file" {
            if let Ok(bytes) = field.bytes().await {
                wasm_bytes = Some(bytes.to_vec());
            }
        }
    }

    // Check if WASM file was provided
    let Some(wasm_bytes) = wasm_bytes else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No WASM file provided".to_string(),
            }),
        )
            .into_response();
    };

    // Try to extract ABI from WASM bytes
    let response = match abi::extract_abi_from_wasm_bytes(&wasm_bytes) {
        Ok(_manifest) => ExportResponse {
            data: serde_json::json!({"has_abi": true}),
            warning: None,
            info: Some(
                "Successfully extracted ABI from WASM file. State values will be decoded using the ABI schema.".to_string()
            ),
        },
        Err(e) => ExportResponse {
            data: serde_json::json!({"has_abi": false}),
            warning: Some(format!(
                "The uploaded WASM file does not contain an exported ABI. The file may not have been built with ABI support. State values will not be decoded. Error: {e}"
            )),
            info: None,
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Validate database path to prevent path traversal attacks
/// Note: This function requires the path to exist so it can resolve symlinks
fn validate_db_path(path: &std::path::Path) -> Result<(), String> {
    // Check for parent directory references in the original path
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(
                "Invalid path: parent directory references (..) are not allowed".to_string(),
            );
        }
    }

    // Canonicalize path to resolve symlinks and get absolute path
    // This helps detect attempts to escape via symlinks
    // Note: This requires the path to exist, so the existence check must happen first
    let canonical_path = path
        .canonicalize()
        .map_err(|e| format!("Invalid path: {e}"))?;

    // Optionally: Add additional checks here if you want to restrict
    // to specific directories. For now, we ensure the path is valid and resolved.
    drop(canonical_path);
    Ok(())
}

fn open_database(path: &PathBuf) -> eyre::Result<DBWithThreadMode<SingleThreaded>> {
    let options = Options::default();

    let cf_names: Vec<String> = Column::all()
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();

    let db = DBWithThreadMode::<SingleThreaded>::open_cf_for_read_only(
        &options, path, &cf_names, false,
    )?;

    Ok(db)
}
