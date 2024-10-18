use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetApplicationDetailsResponse;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(app_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let Ok(app_id_result) = app_id.parse() else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid app id".into(),
        }
        .into_response();
    };

    let application = state
        .ctx_manager
        .get_application(&app_id_result)
        .map_err(|err| parse_api_error(err).into_response());

    #[expect(clippy::option_if_let_else, reason = "Clearer here")]
    match application {
        Ok(application) => match application {
            Some(application) => ApiResponse {
                payload: GetApplicationDetailsResponse::new(application),
            }
            .into_response(),
            None => ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response(),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
