use std::path::PathBuf;

use axum::http::HeaderValue;
use axum::{
    extract::Multipart,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use rocksdb::{DBWithThreadMode, Options, SingleThreaded};
use serde::Serialize;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

use crate::{abi, dag, export, types::Column};
use calimero_wasm_abi::schema::Manifest;
use hex;

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
    // Note: Browser caching can be an issue during development
    // Use hard refresh (Cmd+Shift+R / Ctrl+Shift+R) to bypass cache
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
        .nest_service("/static", serve_static)
        .fallback(render_app) // Fallback to app for any unmatched routes (SPA behavior)
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ));

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
    let mut state_schema_text: Option<String> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();
        eprintln!("DEBUG: Received field: {}", name);

        match name.as_str() {
            "db_path" => {
                if let Ok(value) = field.text().await {
                    eprintln!("DEBUG: db_path value: {}", value);
                    db_path = Some(PathBuf::from(value));
                } else {
                    eprintln!("WARNING: Failed to read db_path as text");
                }
            }
            "state_schema_file" => {
                if let Ok(text) = field.text().await {
                    eprintln!("DEBUG: state_schema_file size: {} chars", text.len());
                    state_schema_text = Some(text);
                } else {
                    eprintln!("WARNING: Failed to read state_schema_file as text");
                }
            }
            "wasm_file" => {
                eprintln!("WARNING: Received 'wasm_file' field - this is deprecated. Please use 'state_schema_file' instead.");
                // Try to read it as text (in case it's actually a JSON schema file)
                if let Ok(text) = field.text().await {
                    eprintln!("WARNING: wasm_file contains text ({} chars), treating as state_schema_file", text.len());
                    // Check if it looks like JSON
                    if text.trim_start().starts_with('{') {
                        eprintln!(
                            "WARNING: wasm_file appears to be JSON, using as state_schema_file"
                        );
                        state_schema_text = Some(text);
                    }
                }
            }
            _ => {
                eprintln!("DEBUG: Ignoring unknown field: {}", name);
            }
        }
    }

    // Validate inputs
    let Some(db_path) = db_path else {
        eprintln!("ERROR: Database path is missing in export request");
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Database path is required".to_owned(),
            }),
        )
            .into_response();
    };

    eprintln!("DEBUG: Export request - db_path: {}", db_path.display());
    eprintln!(
        "DEBUG: Export request - has state_schema: {}",
        state_schema_text.is_some()
    );

    // Check if database path exists first
    if !db_path.exists() {
        eprintln!("ERROR: Database path does not exist: {}", db_path.display());
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
        eprintln!("ERROR: Path validation failed: {}", e);
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response();
    }

    // Load state schema file
    let mut warning_message = None;
    let mut info_message = None;
    let schema = if let Some(schema_text) = state_schema_text {
        match serde_json::from_str::<serde_json::Value>(&schema_text) {
            Ok(schema_value) => match abi::load_state_schema_from_json_value(&schema_value) {
                Ok(schema) => {
                    info_message = Some(
                            "Successfully loaded state schema. State values will be decoded using the schema.".to_string()
                        );
                    Some(schema)
                }
                Err(e) => {
                    let warning = format!("Failed to load state schema. Error: {e}");
                    eprintln!("Warning: {warning}");
                    warning_message = Some(warning);
                    None
                }
            },
            Err(e) => {
                let warning = format!("Failed to parse state schema JSON. Error: {e}");
                eprintln!("Warning: {warning}");
                warning_message = Some(warning);
                None
            }
        }
    } else {
        // Will infer schema after opening database
        None
    };

    // Open database (needed for both schema inference and export)
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

    // Infer schema if not provided (no context_id for global export)
    let schema = if schema.is_none() {
        eprintln!("No state schema file provided - inferring schema from database...");
        match abi::infer_schema_from_database(&db, None) {
            Ok(manifest) => {
                eprintln!("Schema inferred successfully");
                info_message = Some(
                    "No schema file provided - schema inferred from database metadata. State values will be decoded using inferred schema.".to_string()
                );
                Some(manifest)
            }
            Err(e) => {
                let warning = format!(
                    "Failed to infer schema from database: {e}. State values will not be decoded."
                );
                eprintln!("Warning: {warning}");
                warning_message = Some(warning);
                None
            }
        }
    } else {
        schema
    };

    // Export all columns
    let columns = Column::all().to_vec();
    let data = if let Some(schema) = schema {
        match export::export_data(&db, &columns, &schema) {
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
    let mut state_schema_text: Option<String> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();

        match name.as_str() {
            "db_path" => {
                if let Ok(value) = field.text().await {
                    db_path = Some(PathBuf::from(value));
                }
            }
            "state_schema_file" => {
                if let Ok(text) = field.text().await {
                    state_schema_text = Some(text);
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

    // State schema is optional - infer from database if not provided
    let schema = if let Some(schema_text) = state_schema_text {
        match serde_json::from_str::<serde_json::Value>(&schema_text) {
            Ok(schema_value) => match abi::load_state_schema_from_json_value(&schema_value) {
                Ok(manifest) => manifest,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("Failed to load state schema: {e}"),
                        }),
                    )
                        .into_response();
                }
            },
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("Failed to parse state schema JSON: {e}"),
                    }),
                )
                    .into_response();
            }
        }
    } else {
        // Infer schema from database
        eprintln!("[server] No schema file provided, inferring from database...");
        match open_database(&db_path) {
            Ok(db) => match abi::infer_schema_from_database(&db, None) {
                Ok(manifest) => {
                    eprintln!("[server] Schema inferred successfully");
                    manifest
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("Failed to infer schema from database: {e}"),
                        }),
                    )
                        .into_response();
                }
            },
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to open database for schema inference: {e}"),
                    }),
                )
                    .into_response();
            }
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

        match export::extract_context_tree(&db, context_id, &schema) {
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
    let mut state_schema_text: Option<String> = None;
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
            "state_schema_file" => {
                if let Ok(text) = field.text().await {
                    state_schema_text = Some(text);
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

    // State schema is optional - infer from database if not provided
    let schema = if let Some(schema_text) = state_schema_text {
        match serde_json::from_str::<serde_json::Value>(&schema_text) {
            Ok(schema_value) => match abi::load_state_schema_from_json_value(&schema_value) {
                Ok(schema) => schema,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("Failed to load state schema: {e}"),
                        }),
                    )
                        .into_response();
                }
            },
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("Failed to parse state schema JSON: {e}"),
                    }),
                )
                    .into_response();
            }
        }
    } else {
        // Infer schema from database for this specific context
        eprintln!(
            "[server] No schema file provided, inferring from database for context {}...",
            context_id
        );
        match open_database(&db_path) {
            Ok(db) => {
                // Decode context_id from hex string
                let context_id_bytes = match hex::decode(&context_id) {
                    Ok(bytes) if bytes.len() == 32 => bytes,
                    _ => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(ErrorResponse {
                                error: format!("Invalid context_id format: {}", context_id),
                            }),
                        )
                            .into_response();
                    }
                };
                match abi::infer_schema_from_database(&db, Some(&context_id_bytes)) {
                    Ok(manifest) => {
                        let field_count = manifest
                            .state_root
                            .as_ref()
                            .and_then(|root| manifest.types.get(root))
                            .and_then(|ty| {
                                if let calimero_wasm_abi::schema::TypeDef::Record { fields } = ty {
                                    Some(fields.len())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        eprintln!(
                            "[server] Schema inferred successfully for context {}: {} fields found",
                            context_id, field_count
                        );
                        manifest
                    }
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorResponse {
                                error: format!("Failed to infer schema from database: {e}"),
                            }),
                        )
                            .into_response();
                    }
                }
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to open database for schema inference: {e}"),
                    }),
                )
                    .into_response();
            }
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
    let tree_data = match export::extract_context_tree(&db, &context_id, &schema) {
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
    let mut state_schema_text: Option<String> = None;

    // Parse multipart form data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_owned();
        if name.as_str() == "state_schema_file" {
            if let Ok(text) = field.text().await {
                state_schema_text = Some(text);
            }
        }
    }

    // Check if state schema file was provided
    let Some(schema_text) = state_schema_text else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No state schema file provided".to_string(),
            }),
        )
            .into_response();
    };

    // Try to load state schema
    let response = match serde_json::from_str::<serde_json::Value>(&schema_text) {
        Ok(schema_value) => match abi::load_state_schema_from_json_value(&schema_value) {
            Ok(_manifest) => ExportResponse {
                data: serde_json::json!({"has_schema": true}),
                warning: None,
                info: Some(
                    "Successfully loaded state schema. State values will be decoded using the schema.".to_string()
                ),
            },
            Err(e) => ExportResponse {
                data: serde_json::json!({"has_schema": false}),
                warning: Some(format!(
                    "Failed to load state schema. Error: {e}"
                )),
                info: None,
            },
        },
        Err(e) => ExportResponse {
            data: serde_json::json!({"has_schema": false}),
            warning: Some(format!(
                "Failed to parse state schema JSON. Error: {e}"
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
