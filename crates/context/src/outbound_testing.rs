//! `[context.outbound.testing]`: what merobox needs to drive outbound requests
//! end to end without a real service. Compiled only with the `outbound-testing`
//! feature, which merod's test image (`mock-attestation`) enables and a release
//! build never does.
//!
//! - `echo`: a request to a host ending in `.test` is answered in-process with a
//!   JSON echo of what the node would have sent. It reaches the transport only
//!   after every gate in [`OutboundService::prepare`] has passed and the secrets
//!   are filled in, so a scenario sees exactly what left the node, and the echo
//!   goes back through redaction like any answer. Any other host still goes out
//!   over the real transport.
//! - `secrets`: literal values, scoped by package, signer and host as
//!   `env_secrets` are, because a merobox node's environment cannot carry them.

use std::collections::BTreeMap;
use std::sync::Arc;

use eyre::{bail, Result as EyreResult};
use serde_json::json;
use tracing::warn;
use zeroize::Zeroizing;

use super::{
    is_secret_name, normalize_hosts, EnvSecret, EnvSecrets, Prepared, SecretProvider, Transport,
};
use crate::config::{OutboundSettings, OutboundTestSettings};

/// The suffix of a host the echo answers. RFC 2606 reserves `.test`, so no
/// public name ends in it.
const ECHO_SUFFIX: &str = ".test";

/// Swaps in the test secrets and wraps the transport, as `testing` asks.
pub(super) fn apply(
    settings: &OutboundSettings,
    testing: &OutboundTestSettings,
    secrets: &mut Option<Arc<dyn SecretProvider>>,
    transport: &mut Arc<dyn Transport>,
) -> EyreResult<()> {
    if !testing.secrets.is_empty() {
        if !settings.env_secrets.is_empty() || settings.sidecar.is_some() {
            bail!("[context.outbound.testing] secrets exclude env_secrets and a sidecar");
        }
        let mut map = BTreeMap::new();
        for (name, entry) in &testing.secrets {
            if !is_secret_name(name) {
                bail!("outbound test secret name {name:?} must be A-Z, 0-9 and _");
            }
            if entry.packages.is_empty() || entry.signers.is_empty() || entry.hosts.is_empty() {
                bail!("outbound test secret {name} must name packages, signers and hosts");
            }
            let _ = map.insert(
                name.clone(),
                EnvSecret {
                    value: Zeroizing::new(entry.value.clone()),
                    packages: entry.packages.iter().cloned().collect(),
                    signers: entry.signers.iter().cloned().collect(),
                    hosts: normalize_hosts(&entry.hosts)?,
                },
            );
        }
        *secrets = Some(Arc::new(EnvSecrets { secrets: map }));
    }
    if testing.echo {
        *transport = Arc::new(Echo {
            inner: Arc::clone(transport),
        });
    }
    warn!(
        echo = testing.echo,
        secrets = testing.secrets.len(),
        "[context.outbound.testing] is on: a test build; .test hosts are answered in-process"
    );
    Ok(())
}

/// Answers `.test` hosts itself, and hands every other request on.
#[derive(Debug)]
struct Echo {
    inner: Arc<dyn Transport>,
}

impl Transport for Echo {
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        if !request.host.ends_with(ECHO_SUFFIX) {
            return self.inner.send(request);
        }
        let headers: BTreeMap<&str, &str> = request
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let echo = json!({
            "method": format!("{:?}", request.method).to_ascii_uppercase(),
            "url": request.url.as_str(),
            "path": request.url.path(),
            "query": request.url.query(),
            "headers": headers,
            "body": String::from_utf8_lossy(&request.body),
            "secrets": request.secrets.len(),
        });
        Ok((200, serde_json::to_vec(&echo)?))
    }
}

#[cfg(test)]
mod tests {
    use calimero_primitives::outbound::{OutboundMethod, OutboundRequest, OutboundResponse};

    use super::*;
    use crate::config::TestSecretSettings;
    use crate::outbound::{AppIdentity, OutboundService};

    const URL: &str = "https://echo.outbound.test/v1/";

    fn settings(secrets: bool) -> OutboundSettings {
        OutboundSettings {
            enabled: true,
            hosts: vec!["echo.outbound.test".to_owned()],
            testing: Some(OutboundTestSettings {
                echo: true,
                secrets: if secrets {
                    [(
                        "ECHO_KEY".to_owned(),
                        TestSecretSettings {
                            value: "s3cr3t-e2e".to_owned(),
                            packages: vec!["com.calimero.outbound-probe".to_owned()],
                            signers: vec!["did:key:zProbe".to_owned()],
                            hosts: vec!["echo.outbound.test".to_owned()],
                        },
                    )]
                    .into()
                } else {
                    BTreeMap::new()
                },
            }),
            ..OutboundSettings::default()
        }
    }

    #[test]
    fn a_test_host_is_echoed_with_its_secret_filled_in_then_redacted() {
        let service = OutboundService::from_settings(&settings(true))
            .unwrap()
            .unwrap();
        let app = AppIdentity {
            package: "com.calimero.outbound-probe".to_owned(),
            signer: "did:key:zProbe".to_owned(),
        };
        let run = service
            .for_run(app, vec![URL.to_owned()], "ctx".to_owned())
            .unwrap();
        let request = OutboundRequest {
            method: OutboundMethod::Post,
            url: format!("{URL}check?n=1"),
            headers: vec![("X-Key".to_owned(), "{{secret:ECHO_KEY}}".to_owned())],
            body: b"key={{secret:ECHO_KEY}}".to_vec(),
        };
        let answer = run.send(&borsh::to_vec(&request).unwrap()).unwrap();
        let response: OutboundResponse = borsh::from_slice(&answer).unwrap();
        let echo: serde_json::Value = serde_json::from_slice(&response.body).unwrap();

        assert_eq!(echo["method"], "POST");
        assert_eq!(echo["path"], "/v1/check");
        assert_eq!(echo["query"], "n=1");
        assert_eq!(echo["secrets"], 2);
        // Substituted on the way out, removed on the way back.
        assert_eq!(echo["body"], "key=[redacted]");
        assert_eq!(echo["headers"]["X-Key"], "[redacted]");
        assert!(!String::from_utf8_lossy(&response.body).contains("s3cr3t-e2e"));
    }

    #[test]
    fn test_secrets_exclude_the_other_providers() {
        let mut both = settings(true);
        both.env_secrets = [(
            "OTHER".to_owned(),
            crate::config::EnvSecretSettings {
                // Set in every environment, so the env provider itself loads.
                env: "PATH".to_owned(),
                packages: vec!["p".to_owned()],
                signers: vec!["s".to_owned()],
                hosts: vec!["echo.outbound.test".to_owned()],
            },
        )]
        .into();
        assert!(OutboundService::from_settings(&both).is_err());
    }
}
