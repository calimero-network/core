use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{CreateContextRequest, CreateContextResponse};
use tokio::sync::oneshot;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;
