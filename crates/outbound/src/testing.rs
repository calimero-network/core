//! `[outbound.testing]`: what merobox needs to drive outbound requests
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
use std::sync::{Arc, Mutex};

use eyre::{bail, Result as EyreResult};
use serde_json::json;
use tracing::warn;
use zeroize::Zeroizing;

use super::{
    is_secret_name, normalize_hosts, EnvSecret, EnvSecrets, Prepared, SecretProvider, Transport,
};
use crate::{OutboundSettings, OutboundTestSettings};

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
            bail!("[outbound.testing] secrets exclude env_secrets and a sidecar");
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
    if testing.turnstile {
        *transport = Arc::new(Turnstile {
            inner: Arc::clone(transport),
            spent: Mutex::default(),
        });
    }
    warn!(
        echo = testing.echo,
        turnstile = testing.turnstile,
        secrets = testing.secrets.len(),
        "[outbound.testing] is on: a test build; .test hosts are answered in-process"
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

/// Answers Turnstile's siteverify as Cloudflare does for its test secrets
/// (developers.cloudflare.com/turnstile/troubleshooting/testing/), and hands every other
/// request on.
#[derive(Debug)]
struct Turnstile {
    inner: Arc<dyn Transport>,
    /// Each token checked so far, with the idempotency key of its first check.
    spent: Mutex<BTreeMap<String, Option<String>>>,
}

impl Transport for Turnstile {
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        if request.url.as_str() != crate::TURNSTILE_VERIFY_URL {
            return self.inner.send(request);
        }
        let form: BTreeMap<String, String> = url::form_urlencoded::parse(&request.body)
            .into_owned()
            .collect();
        let field = |name: &str| form.get(name).cloned().unwrap_or_default();
        let (secret, token) = (field("secret"), field("response"));
        let key = form.get("idempotency_key").cloned();
        let error = if token.is_empty() {
            Some("missing-input-response")
        } else {
            match secret.as_str() {
                "1x0000000000000000000000000000000AA" => None,
                "2x0000000000000000000000000000000AA" => Some("invalid-input-response"),
                "3x0000000000000000000000000000000AA" => Some("timeout-or-duplicate"),
                _ => Some("invalid-input-secret"),
            }
        };
        let error = error.or_else(|| {
            let mut spent = self.spent.lock().unwrap_or_else(|e| e.into_inner());
            match spent.get(&token) {
                None => {
                    let _ = spent.insert(token, key);
                    None
                }
                Some(first) if first.is_some() && *first == key => None,
                Some(_) => Some("timeout-or-duplicate"),
            }
        });
        let answer = json!({
            "success": error.is_none(),
            "error-codes": error.into_iter().collect::<Vec<_>>(),
        });
        Ok((200, serde_json::to_vec(&answer)?))
    }
}

#[cfg(test)]
mod tests {
    use calimero_primitives::outbound::{OutboundMethod, OutboundRequest, OutboundResponse};

    use super::*;
    use crate::TestSecretSettings;
    use crate::{AppIdentity, Approval, OutboundService};

    const URL: &str = "https://echo.outbound.test/v1/";

    fn settings(secrets: bool) -> OutboundSettings {
        OutboundSettings {
            enabled: true,
            hosts: vec!["echo.outbound.test".to_owned()],
            testing: Some(OutboundTestSettings {
                echo: true,
                turnstile: false,
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
            .for_run(
                app,
                vec![URL.to_owned()],
                Some(Approval {
                    prefixes: vec![URL.to_owned()],
                    reviewed: vec![URL.to_owned()],
                }),
                "ctx".to_owned(),
            )
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
            crate::EnvSecretSettings {
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

    #[test]
    fn turnstile_answers_as_cloudflare_does_for_its_test_secrets() {
        const PASS: &str = "1x0000000000000000000000000000000AA";
        let service = OutboundService::from_settings(&OutboundSettings {
            enabled: true,
            hosts: vec!["challenges.cloudflare.com".to_owned()],
            testing: Some(OutboundTestSettings {
                turnstile: true,
                secrets: [(
                    "TURNSTILE_SECRET".to_owned(),
                    TestSecretSettings {
                        value: PASS.to_owned(),
                        packages: vec!["p".to_owned()],
                        signers: vec!["s".to_owned()],
                        hosts: vec!["challenges.cloudflare.com".to_owned()],
                    },
                )]
                .into(),
                ..OutboundTestSettings::default()
            }),
            ..OutboundSettings::default()
        })
        .unwrap()
        .unwrap();
        let url = crate::TURNSTILE_VERIFY_URL.to_owned();
        let run = service
            .for_run(
                AppIdentity {
                    package: "p".to_owned(),
                    signer: "s".to_owned(),
                },
                vec![url.clone()],
                Some(Approval {
                    prefixes: vec![url.clone()],
                    reviewed: vec![url.clone()],
                }),
                "ctx".to_owned(),
            )
            .unwrap();
        let check = |token: &str, key: &str| {
            let request = OutboundRequest {
                method: OutboundMethod::Post,
                url: url.clone(),
                headers: Vec::new(),
                body: format!(
                    "secret={{{{secret:TURNSTILE_SECRET}}}}&response={token}&idempotency_key={key}"
                )
                .into_bytes(),
            };
            let answer = run.send(&borsh::to_vec(&request).unwrap()).unwrap();
            let response: OutboundResponse = borsh::from_slice(&answer).unwrap();
            serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()
        };
        assert_eq!(check("t1", "k1")["success"], true);
        // The same check again (a retry) answers the same; another spends nothing new.
        assert_eq!(check("t1", "k1")["success"], true);
        let again = check("t1", "k2");
        assert_eq!(again["success"], false);
        assert_eq!(again["error-codes"][0], "timeout-or-duplicate");
        assert_eq!(check("", "k3")["error-codes"][0], "missing-input-response");
    }
}
