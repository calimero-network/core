//! Drives `env::http_request` from every kind of run the node must tell apart,
//! for the `outbound` merobox scenario. Each write stores what came back in
//! `last`, so peers see the outcome replicate without making the request.

use calimero_sdk::outbound::{OutboundMethod, OutboundRequest};
use calimero_sdk::{app, env};
use calimero_storage::collections::LwwRegister;

/// The prefix the manifest declares.
const ECHO: &str = "https://echo.outbound.test/v1/";

#[app::state]
pub struct OutboundProbe {
    /// The outcome of the last request a write made: `<status> <body>`, or
    /// `refused: <reason>`.
    last: LwwRegister<String>,
}

#[app::logic]
impl OutboundProbe {
    #[app::init]
    pub fn init() -> OutboundProbe {
        OutboundProbe {
            last: LwwRegister::new(String::new()),
        }
    }

    /// POSTs `body` to the declared prefix plus `path`, with the node's
    /// `ECHO_KEY` secret in a header and in the body, and stores the outcome.
    pub fn probe(&mut self, path: String, body: String) -> app::Result<String> {
        let outcome = outcome(env::http_request(&OutboundRequest {
            method: OutboundMethod::Post,
            url: format!("{ECHO}{path}"),
            headers: vec![
                ("X-Key".to_owned(), "{{secret:ECHO_KEY}}".to_owned()),
                ("Content-Type".to_owned(), "text/plain".to_owned()),
            ],
            body: format!("{body}&key={{{{secret:ECHO_KEY}}}}").into_bytes(),
        }));
        self.last.set(outcome.clone());
        Ok(outcome)
    }

    /// GETs any URL, declared or not, and stores the outcome.
    pub fn probe_url(&mut self, url: String) -> app::Result<String> {
        let outcome = outcome(env::http_request(&OutboundRequest {
            method: OutboundMethod::Get,
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }));
        self.last.set(outcome.clone());
        Ok(outcome)
    }

    /// Writes, then makes two requests. The node traps the second, so the run
    /// fails and its write is never committed.
    pub fn probe_twice(&mut self) -> app::Result<String> {
        self.last
            .set("written before the second request".to_owned());
        let _first = env::http_request(&get(ECHO));
        let _second = env::http_request(&get(ECHO));
        Ok("unreachable: the second request traps".to_owned())
    }

    /// A view asking for a request: the node gives a view no client, so the
    /// call traps.
    pub fn probe_in_view(&self) -> app::Result<String> {
        Ok(outcome(env::http_request(&get(ECHO))))
    }

    /// The outcome the last write stored.
    pub fn last(&self) -> app::Result<String> {
        Ok(self.last.get().clone())
    }
}

fn get(url: &str) -> OutboundRequest {
    OutboundRequest {
        method: OutboundMethod::Get,
        url: url.to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn outcome(result: Result<calimero_sdk::outbound::OutboundResponse, String>) -> String {
    match result {
        Ok(response) => format!(
            "{} {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ),
        Err(reason) => format!("refused: {reason}"),
    }
}
