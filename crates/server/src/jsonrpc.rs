use axum::response::IntoResponse;
use axum::routing::{get_service, MethodRouter};
use tracing::info;

use service::CalimeroRPCServer;
mod service;

pub(crate) fn service(
    config: &crate::config::ServerConfig,
) -> eyre::Result<Option<(&'static str, MethodRouter)>> {
    let (stop_handle, _server_handle) = jsonrpsee::server::stop_channel();
    let service_builder = jsonrpsee::server::ServerBuilder::new().to_service_builder();

    // let service = service_builder.build(service::CalimeroRPCImpl::new().into_rpc(), stop_handle);

    let server = service_builder.build(service::CalimeroRPCImpl::new().into_rpc(), stop_handle);

    let path = "/rpc"; // todo! source from config

    for listen in config.listen.iter() {
        info!("WebSocket server listening on {}/ws{{{}}}", listen, path);
    }

    Ok(Some((
        path,
        get_service(server).handle_error(
            |err: Box<dyn std::error::Error + Send + Sync>| async move {
                err.to_string().into_response()
            },
        ),
    )))
}
