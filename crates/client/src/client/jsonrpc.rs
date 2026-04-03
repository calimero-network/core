//! JSON-RPC API methods for the Calimero client.

use calimero_server_primitives::jsonrpc::{Request, Response};
use eyre::Result;
use serde::Serialize;

use super::Client;
use crate::traits::{ClientAuthenticator, ClientStorage};

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn execute_jsonrpc<P>(&self, request: Request<P>) -> Result<Response>
    where
        P: Serialize,
    {
        // Debug: Print the request being sent
        eprintln!(
            "🔍 JSON-RPC Request to {}: {}",
            self.connection.api_url.join("jsonrpc")?,
            serde_json::to_string_pretty(&request)?
        );

        let response = self.connection.post("jsonrpc", request).await?;

        // Debug: Print the parsed response
        eprintln!(
            "🔍 JSON-RPC Parsed Response: {}",
            serde_json::to_string_pretty(&response)?
        );

        Ok(response)
    }
}
