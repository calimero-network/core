use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    GetApplicationDetailsResponse, GetApplicationResponse, InstallApplicationRequest,
    InstallApplicationResponse, InstallDevApplicationRequest, ListApplicationsResponse,
};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn install_dev_application_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallDevApplicationRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .install_application_from_path(req.path, req.metadata)
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn get_application(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
) -> impl IntoResponse {
    match state.ctx_manager.get_application(&application_id) {
        Ok(application) => ApiResponse {
            payload: GetApplicationResponse::new(application),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn install_application_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallApplicationRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .install_application_from_url(req.url, req.metadata /*, req.hash */)
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn list_applications_handler(
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let applications = state
        .ctx_manager
        .list_installed_applications()
        .map_err(|err| parse_api_error(err).into_response());
    match applications {
        Ok(applications) => {
            ApiResponse {
                payload: ListApplicationsResponse::new(applications),
            }
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}

pub async fn get_application_details_handler(
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

    #[allow(clippy::option_if_let_else)]
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
