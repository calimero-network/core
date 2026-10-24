//! Every gate of an outbound request, against a fake transport.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use calimero_primitives::outbound::{OutboundMethod, OutboundRequest, OutboundResponse};

use super::*;

const VERIFY: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const HOST: &str = "challenges.cloudflare.com";
const SECRET: &str = "0x4AAA-very-secret";

fn forms() -> AppIdentity {
    AppIdentity {
        package: "com.calimero.mero-forms".to_owned(),
        signer: "did:key:zForms".to_owned(),
    }
}

/// One secret, `TURNSTILE_SECRET`, for mero-forms only, bound to Cloudflare.
#[derive(Debug)]
struct OneSecret;

impl SecretProvider for OneSecret {
    fn secret(&self, app: &AppIdentity, name: &str) -> EyreResult<Option<Secret>> {
        Ok(
            (name == "TURNSTILE_SECRET" && app.package == "com.calimero.mero-forms").then(|| {
                Secret {
                    value: Zeroizing::new(SECRET.to_owned()),
                    hosts: vec![HOST.to_owned()],
                }
            }),
        )
    }
}

/// Records what it was asked to send and answers with a body that echoes the
/// secret, as a careless API might.
#[derive(Debug, Default)]
struct Fake(Mutex<Vec<Sent>>);

/// The URL, body and headers of a request the fake transport was given.
type Sent = (String, Vec<u8>, Vec<(String, String)>);

impl Transport for Fake {
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        self.0.lock().unwrap().push((
            request.url.to_string(),
            request.body.to_vec(),
            request
                .headers
                .iter()
                .map(|(n, v)| (n.clone(), v.to_string()))
                .collect(),
        ));
        Ok((
            200,
            format!(r#"{{"success":true,"echo":"{SECRET}"}}"#).into_bytes(),
        ))
    }
}

fn service(fake: &Arc<Fake>) -> Arc<OutboundService> {
    OutboundService::with_parts(&[HOST], Some(Arc::new(OneSecret)), fake.clone())
}

fn declared() -> BTreeSet<String> {
    BTreeSet::from([VERIFY.to_owned()])
}

fn post(url: &str, body: &str) -> OutboundRequest {
    OutboundRequest {
        method: OutboundMethod::Post,
        url: url.to_owned(),
        headers: vec![(
            "Content-Type".to_owned(),
            "application/x-www-form-urlencoded".to_owned(),
        )],
        body: body.as_bytes().to_vec(),
    }
}

fn refusal(service: &OutboundService, request: OutboundRequest) -> String {
    service
        .prepare(&forms(), &declared(), request)
        .expect_err("expected a refusal")
        .to_string()
}

#[test]
fn a_declared_request_reaches_the_host_with_its_secret_and_comes_back_redacted() {
    let fake = Arc::new(Fake::default());
    let run = service(&fake)
        .for_run(forms(), vec![VERIFY.to_owned()], "ctx".to_owned())
        .expect("the app declares a URL");
    let request = post(VERIFY, "secret={{secret:TURNSTILE_SECRET}}&response=tok");
    let answer = run.send(&borsh::to_vec(&request).unwrap()).expect("sent");

    let sent = fake.0.lock().unwrap();
    assert_eq!(sent[0].0, VERIFY);
    assert_eq!(
        sent[0].1,
        format!("secret={SECRET}&response=tok").into_bytes()
    );

    let response = OutboundResponse::try_from_slice(&answer).unwrap();
    assert_eq!(response.status, 200);
    let body = String::from_utf8(response.body).unwrap();
    assert!(
        !body.contains(SECRET),
        "the secret never reaches the app: {body}"
    );
    assert!(body.contains("[redacted]"));
}

#[test]
fn only_urls_under_a_declared_prefix() {
    let fake = Arc::new(Fake::default());
    let s = service(&fake);
    // The prefix itself, with a query string, and a path below it.
    assert!(s.prepare(&forms(), &declared(), post(VERIFY, "")).is_ok());
    assert!(s
        .prepare(&forms(), &declared(), post(&format!("{VERIFY}?x=1"), ""))
        .is_ok());
    assert!(s
        .prepare(&forms(), &declared(), post(&format!("{VERIFY}/v2"), ""))
        .is_ok());
    // Not at a `/`, beside it, above it, or out of it through `..`.
    let outside = [
        format!("{VERIFY}2"),
        "https://challenges.cloudflare.com/other".to_owned(),
        "https://challenges.cloudflare.com/turnstile/v0".to_owned(),
        format!("{VERIFY}/../../admin"),
        format!("{VERIFY}/%2e%2e/%2e%2e/admin"),
    ];
    for url in outside {
        assert!(
            refusal(&s, post(&url, "")).contains("no URL prefix"),
            "{url}"
        );
    }
    assert!(refusal(&s, post(&format!("{VERIFY}#x"), "")).contains("fragment"));
    // An app that declares nothing usable gets no client at all.
    assert!(s.for_run(forms(), vec![], "ctx".to_owned()).is_none());
    for unusable in [
        "http://challenges.cloudflare.com/x",
        "https://challenges.cloudflare.com/x?key=1",
        "https://10.0.0.1/x",
    ] {
        assert!(s
            .for_run(forms(), vec![unusable.to_owned()], "ctx".to_owned())
            .is_none());
    }
}

#[test]
fn any_method_and_a_star_allows_every_host_an_app_declares() {
    let fake = Arc::new(Fake::default());
    let s = OutboundService::with_parts(&["*"], Some(Arc::new(OneSecret)), fake);
    let api = "https://api.example.com/v1/";
    let mut r = post("https://api.example.com/v1/customers/cus_1", "{}");
    for method in [
        OutboundMethod::Post,
        OutboundMethod::Put,
        OutboundMethod::Patch,
        OutboundMethod::Delete,
    ] {
        r.method = method;
        assert!(s
            .prepare(&forms(), &BTreeSet::from([api.to_owned()]), r.clone())
            .is_ok());
    }
    // The secret is still bound to its own host.
    let leaked = s
        .prepare(
            &forms(),
            &BTreeSet::from([api.to_owned()]),
            post(
                "https://api.example.com/v1/x",
                "{{secret:TURNSTILE_SECRET}}",
            ),
        )
        .unwrap_err()
        .to_string();
    assert!(leaked.contains("may not be sent"), "{leaked}");
}

#[test]
fn only_hosts_the_operator_lists_by_name_over_https_on_443() {
    let fake = Arc::new(Fake::default());
    let s = service(&fake);
    let refuse = |url: &str| {
        s.prepare(&forms(), &BTreeSet::from([url.to_owned()]), post(url, ""))
            .unwrap_err()
            .to_string()
    };
    assert!(
        refuse("https://evil.example/collect").contains("not a host this node's operator allows")
    );
    assert!(refuse("http://challenges.cloudflare.com/x").contains("only https"));
    assert!(refuse("https://challenges.cloudflare.com:8443/x").contains("port 443"));
    assert!(refuse("https://169.254.169.254/latest").contains("DNS name"));
    assert!(refuse("https://u:p@challenges.cloudflare.com/x").contains("credentials"));
}

#[test]
fn a_secret_goes_only_to_its_own_hosts_and_only_for_its_own_app() {
    let fake = Arc::new(Fake::default());
    // The operator allows a second host, but the secret is not bound to it.
    let s =
        OutboundService::with_parts(&[HOST, "api.example.com"], Some(Arc::new(OneSecret)), fake);
    let url = "https://api.example.com/hook";
    let leaked = s
        .prepare(
            &forms(),
            &BTreeSet::from([url.to_owned()]),
            post(url, "{{secret:TURNSTILE_SECRET}}"),
        )
        .unwrap_err()
        .to_string();
    assert!(
        leaked.contains("may not be sent to api.example.com"),
        "{leaked}"
    );

    let other = AppIdentity {
        package: "com.example.other".to_owned(),
        signer: "did:key:zOther".to_owned(),
    };
    let stolen = s
        .prepare(
            &other,
            &declared(),
            post(VERIFY, "{{secret:TURNSTILE_SECRET}}"),
        )
        .unwrap_err()
        .to_string();
    assert!(
        stolen.contains("no secret TURNSTILE_SECRET for com.example.other"),
        "{stolen}"
    );
}

#[test]
fn headers_and_bodies_are_bounded() {
    let fake = Arc::new(Fake::default());
    let s = service(&fake);
    let mut r = post(VERIFY, "");
    r.headers.push(("Host".to_owned(), "internal".to_owned()));
    assert!(refusal(&s, r).contains("may not be set"));

    let mut r = post(VERIFY, "");
    r.headers
        .push(("X-Bad".to_owned(), "a\r\nInjected: 1".to_owned()));
    assert!(refusal(&s, r).contains("malformed"));

    let r = post(VERIFY, &"x".repeat(MAX_REQUEST_BODY + 1));
    assert!(refusal(&s, r).contains("longer than"));

    let mut r = post(VERIFY, "body");
    r.method = OutboundMethod::Get;
    assert!(refusal(&s, r).contains("a GET has no body"));

    assert!(refusal(&s, post(VERIFY, "{{secret:TURNSTILE_SECRET")).contains("unterminated"));
    assert!(refusal(&s, post(VERIFY, "{{secret:lower}}")).contains("must be A-Z"));
}

#[test]
fn without_a_secret_provider_a_request_naming_one_is_refused() {
    let fake = Arc::new(Fake::default());
    let s = OutboundService::with_parts(&[HOST], None, fake);
    assert!(refusal(&s, post(VERIFY, "{{secret:TURNSTILE_SECRET}}")).contains("no secrets"));
    // A request with no secret still goes.
    assert!(s.prepare(&forms(), &declared(), post(VERIFY, "ok")).is_ok());
}

#[test]
fn only_public_addresses() {
    for private in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "0.0.0.0",
        "224.0.0.1",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:10.0.0.1",
        "64:ff9b::a00:1",
    ] {
        assert!(!is_public(private.parse::<IpAddr>().unwrap()), "{private}");
    }
    for public in ["1.1.1.1", "104.16.0.1", "2606:4700::1111"] {
        assert!(is_public(public.parse::<IpAddr>().unwrap()), "{public}");
    }
}

#[test]
fn the_operator_config_is_checked_at_start() {
    let off = OutboundSettings::default();
    assert!(OutboundService::from_settings(&off).unwrap().is_none());

    let no_hosts = OutboundSettings {
        enabled: true,
        ..OutboundSettings::default()
    };
    assert!(OutboundService::from_settings(&no_hosts).is_err());

    let bad_sidecar = OutboundSettings {
        enabled: true,
        hosts: vec![HOST.to_owned()],
        sidecar: Some(crate::config::SidecarSettings {
            url: "http://secrets.example.com/v1".to_owned(),
            token_env: "UNUSED".to_owned(),
        }),
        ..OutboundSettings::default()
    };
    let err = OutboundService::from_settings(&bad_sidecar)
        .unwrap_err()
        .to_string();
    assert!(err.contains("loopback"), "{err}");

    let missing_env = OutboundSettings {
        enabled: true,
        hosts: vec![HOST.to_owned()],
        env_secrets: [(
            "TURNSTILE_SECRET".to_owned(),
            crate::config::EnvSecretSettings {
                env: "CALIMERO_OUTBOUND_TEST_UNSET_VARIABLE".to_owned(),
                packages: vec!["com.calimero.mero-forms".to_owned()],
                signers: vec!["did:key:zForms".to_owned()],
                hosts: vec![HOST.to_owned()],
            },
        )]
        .into(),
        ..OutboundSettings::default()
    };
    let err = OutboundService::from_settings(&missing_env)
        .unwrap_err()
        .to_string();
    assert!(err.contains("is unset"), "{err}");
}

#[test]
fn an_app_declares_urls_through_its_installed_metadata() {
    use calimero_primitives::application::{
        Application, ApplicationBlob, ApplicationId, ApplicationSource,
    };
    use calimero_primitives::blobs::BlobId;

    let mut app = Application::new(
        ApplicationId::from([1; 32]),
        ApplicationBlob {
            bytecode: BlobId::from([2; 32]),
            compiled: BlobId::from([3; 32]),
        },
        0,
        "calimero://test".parse::<ApplicationSource>().unwrap(),
        serde_json::to_vec(&serde_json::json!({ "outbound": [VERIFY] })).unwrap(),
    );
    // Without a package and a signer the node cannot tell whose app it is.
    assert!(app_outbound(&app).is_none());
    app.package = "com.calimero.mero-forms".to_owned();
    app.signer_id = Some("did:key:zForms".parse().unwrap());
    let (identity, declared) = app_outbound(&app).unwrap();
    assert_eq!(identity, forms());
    assert_eq!(declared, vec![VERIFY.to_owned()]);
}

#[test]
fn an_env_secret_is_for_its_packages_built_by_its_signers_only() {
    const VAR: &str = "CALIMERO_OUTBOUND_TEST_SIGNER_SCOPED";
    // No other test reads or writes this variable.
    std::env::set_var(VAR, SECRET);
    let settings = OutboundSettings {
        enabled: true,
        hosts: vec![HOST.to_owned()],
        env_secrets: [(
            "TURNSTILE_SECRET".to_owned(),
            crate::config::EnvSecretSettings {
                env: VAR.to_owned(),
                packages: vec!["com.calimero.mero-forms".to_owned()],
                signers: vec!["did:key:zForms".to_owned()],
                hosts: vec![HOST.to_owned()],
            },
        )]
        .into(),
        ..OutboundSettings::default()
    };
    let secrets = EnvSecrets::from_settings(&settings).unwrap();
    assert!(secrets
        .secret(&forms(), "TURNSTILE_SECRET")
        .unwrap()
        .is_some());

    // The same package name, built and signed by someone else.
    let impostor = AppIdentity {
        signer: "did:key:zImpostor".to_owned(),
        ..forms()
    };
    assert!(secrets
        .secret(&impostor, "TURNSTILE_SECRET")
        .unwrap()
        .is_none());

    let unsigned = OutboundSettings {
        env_secrets: [(
            "TURNSTILE_SECRET".to_owned(),
            crate::config::EnvSecretSettings {
                env: VAR.to_owned(),
                packages: vec!["com.calimero.mero-forms".to_owned()],
                signers: vec![],
                hosts: vec![HOST.to_owned()],
            },
        )]
        .into(),
        ..settings
    };
    let err = EnvSecrets::from_settings(&unsigned)
        .unwrap_err()
        .to_string();
    assert!(err.contains("signers"), "{err}");
}
