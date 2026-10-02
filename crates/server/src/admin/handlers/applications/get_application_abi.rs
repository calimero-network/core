use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_node_primitives::client::application::NotABundle;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{GetApplicationAbiQuery, GetApplicationAbiResponse};
use calimero_wasm_abi::embed::{read_embedded_state_schema_versioned, EmbeddedSchema};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(application_id): Path<ApplicationId>,
    Query(query): Query<GetApplicationAbiQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    info!(application_id=%application_id, service_name=?query.service_name, "Getting application ABI");

    let application = match state.node_client.get_application(&application_id) {
        Ok(Some(application)) => application,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Application not found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(application_id=%application_id, error=?err, "Failed to get application");
            return parse_api_error(err).into_response();
        }
    };

    let service_names: Vec<String> = application.services.keys().cloned().collect();
    let service = match resolve_service(&service_names, query.service_name.as_deref()) {
        Ok(service) => service,
        Err(message) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message,
            }
            .into_response();
        }
    };

    let wasm = match state
        .node_client
        .application_bytes_from_blob(&application.blob.bytecode, service.as_deref())
        .await
    {
        Ok(Some(wasm)) => wasm,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Application bytecode not found".to_owned(),
            }
            .into_response();
        }
        Err(err) if err.downcast_ref::<NotABundle>().is_some() => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message:
                    "application is raw wasm, which never runs; reinstall it as a signed bundle"
                        .to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(application_id=%application_id, error=?err, "Failed to load application bytecode");
            return parse_api_error(err).into_response();
        }
    };

    match read_embedded_state_schema_versioned(&wasm) {
        EmbeddedSchema::Supported(manifest) => match serde_json::to_value(&manifest) {
            Ok(abi) => ApiResponse {
                payload: GetApplicationAbiResponse::new(abi),
            }
            .into_response(),
            Err(err) => {
                error!(application_id=%application_id, error=?err, "Failed to serialize ABI manifest");
                parse_api_error(err.into()).into_response()
            }
        },
        EmbeddedSchema::UnsupportedVersion(version) => ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("unsupported ABI schema version {version}"),
        }
        .into_response(),
        EmbeddedSchema::Absent => ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "application has no usable embedded ABI (absent or malformed); rebuild it with `cargo mero build`"
                .to_owned(),
        }
        .into_response(),
    }
}

/// Picks the service whose wasm carries the ABI, validating against the app
/// record so bad requests 400 with the available names instead of 500ing.
fn resolve_service(names: &[String], requested: Option<&str>) -> Result<Option<String>, String> {
    let available = || names.join(", ");
    match requested {
        None if names.is_empty() => Ok(None),
        None if names.len() == 1 => Ok(Some(names[0].clone())),
        None => Err(format!(
            "application has multiple services; pass service_name (available: {})",
            available()
        )),
        Some(_) if names.is_empty() => {
            Err("application has no named services; omit service_name".to_owned())
        }
        Some(name) if names.iter().any(|n| n == name) => Ok(Some(name.to_owned())),
        Some(name) => Err(format!(
            "service \"{name}\" not found (available: {})",
            available()
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::{Path, Query};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Extension;
    use calimero_primitives::application::ApplicationId;
    use calimero_server_primitives::admin::GetApplicationAbiQuery;
    use calimero_store::db::InMemoryDB;
    use calimero_store::{key, types, Store};

    use super::{handler, resolve_service};

    /// Raw wasm never runs, so its ABI is the caller's problem to fix, not a fault.
    #[actix::test]
    async fn a_raw_wasm_row_is_a_bad_request() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (state, _blobs) = crate::test_support::admin_state(&store).await;
        let raw = b"raw wasm, not a bundle";
        let (blob_id, size) = state
            .node_client
            .add_blob(&raw[..], Some(raw.len() as u64), None)
            .await
            .expect("store the bytes");
        let application_id = ApplicationId::from([0x5A; 32]);
        store
            .handle()
            .put(
                &key::ApplicationMeta::new(application_id),
                &types::ApplicationMeta::new(
                    key::BlobMeta::new(blob_id),
                    size,
                    "calimero://pending-blob-share".into(),
                    Box::default(),
                    key::BlobMeta::new([0; 32].into()),
                    types::PackageInfo {
                        package: "".into(),
                        version: "".into(),
                        signer_id: "".into(),
                        state_version: 0,
                    },
                ),
            )
            .expect("a raw row");

        let response = handler(
            Path(application_id),
            Query(GetApplicationAbiQuery { service_name: None }),
            Extension(state),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_services_and_no_request_targets_the_default_blob() {
        assert_eq!(resolve_service(&[], None), Ok(None));
    }

    #[test]
    fn no_services_but_a_requested_name_is_rejected() {
        let err = resolve_service(&[], Some("api")).unwrap_err();
        assert!(err.contains("has no named services"), "got: {err}");
    }

    #[test]
    fn a_single_service_is_selected_implicitly() {
        let names = names(&["api"]);
        assert_eq!(resolve_service(&names, None), Ok(Some("api".to_owned())));
    }

    #[test]
    fn multiple_services_without_a_name_lists_them() {
        let names = names(&["api", "worker"]);
        let err = resolve_service(&names, None).unwrap_err();
        assert!(err.contains("api") && err.contains("worker"), "got: {err}");
    }

    #[test]
    fn a_known_name_is_selected() {
        let names = names(&["api", "worker"]);
        assert_eq!(
            resolve_service(&names, Some("worker")),
            Ok(Some("worker".to_owned()))
        );
    }

    #[test]
    fn an_unknown_name_lists_what_exists() {
        let names = names(&["api", "worker"]);
        let err = resolve_service(&names, Some("web")).unwrap_err();
        assert!(
            err.contains("web") && err.contains("api") && err.contains("worker"),
            "got: {err}"
        );
    }
}
