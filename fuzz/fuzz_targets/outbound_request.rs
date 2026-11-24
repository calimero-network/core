#![no_main]
//! An app's outbound request: a run hands its node a borsh `OutboundRequest`
//! (`OutboundHost::send`), which the node decodes, checks against the
//! operator's hosts, the app's declared prefixes and the approval, and fills
//! with secrets, all on bytes the app chose.
//!
//! The input is the request. The transport records what reaches it and echoes
//! its body: only an https request to the allowed host, under the declared
//! and approved prefix, with headers that cannot split a line, gets there, a
//! secret goes only to the host it is bound to, and the app never reads a
//! secret back unless it sent it itself.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use calimero_outbound::fuzz_api::service;
use calimero_outbound::{
    AppIdentity, Approval, OutboundHost, OwnSecrets, Prepared, Secret, SecretProvider, Transport,
};
use calimero_primitives::outbound::OutboundResponse;
use libfuzzer_sys::fuzz_target;
use zeroize::Zeroizing;

const HOST: &str = "api.example.com";
const PREFIX: &str = "https://api.example.com/v1/";
const SECRET: &str = "fuzz-secret-value-8c1f";

#[derive(Debug)]
struct Secrets;

impl SecretProvider for Secrets {
    fn secret(&self, _app: &AppIdentity, name: &str) -> eyre::Result<Option<Secret>> {
        Ok((name == "API_KEY").then(|| Secret {
            value: Zeroizing::new(SECRET.to_owned()),
            hosts: vec![HOST.to_owned()],
        }))
    }
}

/// Records what reaches the network and echoes the body and header values.
#[derive(Debug, Default)]
struct Echo {
    sent: Mutex<Vec<(String, String, Vec<(String, String)>)>>,
}

impl Transport for Echo {
    fn send(&self, request: &Prepared) -> eyre::Result<(u16, Vec<u8>)> {
        let headers: Vec<(String, String)> = request
            .headers
            .iter()
            .map(|(n, v)| (n.clone(), v.as_str().to_owned()))
            .collect();
        let mut echo = request.body.to_vec();
        // Apart, so no secret forms across two parts: a header value holds no NUL.
        for (_, value) in &headers {
            echo.push(0);
            echo.extend_from_slice(value.as_bytes());
        }
        self.sent.lock().expect("not poisoned").push((
            request.url.as_str().to_owned(),
            request.host.clone(),
            headers,
        ));
        Ok((200, echo))
    }
}

fuzz_target!(|data: &[u8]| {
    let echo = Arc::new(Echo::default());
    let host = service(&[HOST], Some(Arc::new(Secrets)), echo.clone())
        .for_run(
            AppIdentity {
                package: "network.calimero.fuzz".into(),
                signer: "did:key:fuzz".into(),
                secrets: None,
                own: OwnSecrets(BTreeMap::new()),
            },
            vec![PREFIX.into()],
            Some(Approval {
                prefixes: vec![PREFIX.into()],
                reviewed: vec![PREFIX.into()],
            }),
            "fuzz".into(),
        )
        .expect("the app declares a usable prefix");

    let answer = host.send(data);
    let sent = std::mem::take(&mut *echo.sent.lock().expect("not poisoned"));
    assert!(
        sent.len() <= 1,
        "one request reached the network more than once"
    );
    let Ok(answer) = answer else {
        assert!(sent.is_empty(), "a refused request reached the network");
        return;
    };
    let [(url, host, headers)] = sent.as_slice() else {
        panic!("an answered request never reached the network");
    };
    let parsed = url::Url::parse(url).expect("the transport got a URL");
    assert_eq!(parsed.scheme(), "https");
    assert_eq!(parsed.port_or_known_default(), Some(443));
    assert_eq!(host, HOST);
    assert_eq!(parsed.host_str(), Some(HOST));
    assert!(
        parsed.path().starts_with("/v1/"),
        "{url} is outside the declared prefix"
    );
    assert!(parsed.fragment().is_none() && parsed.username().is_empty());
    for (name, value) in headers {
        assert!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
        assert!(!value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0));
    }
    let response: OutboundResponse = borsh::from_slice(&answer).expect("the node answers borsh");
    if !contains(data, SECRET.as_bytes()) {
        assert!(
            !contains(&response.body, SECRET.as_bytes()),
            "the app read back a secret it never had"
        );
    }
});

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
