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

use crate::{abi, export, types::Column};

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Serialize)]
struct ExportResponse {
    data: serde_json::Value,
}

pub async fn start_gui_server(port: u16) -> eyre::Result<()> {
    let app = Router::new()
        .route("/", get(render_app))
        .route("/api/export", post(handle_export));

    let addr = format!("127.0.0.1:{}", port);
    println!("Starting GUI server at http://{}", addr);
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

    let Some(wasm_bytes) = wasm_bytes else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "WASM file is required".to_owned(),
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

    // Export all columns
    let columns = Column::all().to_vec();
    let data = match export::export_data(&db, &columns, &abi_manifest) {
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
    };

    (StatusCode::OK, Json(ExportResponse { data })).into_response()
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
