use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{
    ApplicationInstallResult, ApplicationListResult, GetApplicationDetailsResponse,
    GetApplicationResponse, GetApplicationResult, InstallApplicationResponse,
    ListApplicationsResponse,
};

use crate::admin::service::{parse_api_error, AdminState, ApiError, ApiResponse};

pub async fn install_dev_application_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<calimero_server_primitives::admin::InstallDevApplicationRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .install_application_from_path(req.path, req.version, req.metadata)
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse {
                data: ApplicationInstallResult { application_id },
            },
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn get_application(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<calimero_primitives::application::ApplicationId>,
) -> impl IntoResponse {
    match state.ctx_manager.get_application(&application_id) {
        Ok(application) => ApiResponse {
            payload: GetApplicationResponse {
                data: GetApplicationResult { application },
            },
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn install_application_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<calimero_server_primitives::admin::InstallApplicationRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .install_application_from_url(req.url, req.version, req.metadata /*, req.hash */)
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse {
                data: ApplicationInstallResult { application_id },
            },
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
                payload: ListApplicationsResponse {
                    data: ApplicationListResult { apps: applications },
                },
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
    let app_id_result = match app_id.parse() {
        Ok(app_id) => app_id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid app id".into(),
            }
            .into_response();
        }
    };

    let application = state
        .ctx_manager
        .get_application(&app_id_result)
        .map_err(|err| parse_api_error(err).into_response());

    match application {
        Ok(application) => match application {
            Some(application) => ApiResponse {
                payload: GetApplicationDetailsResponse { data: application },
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
