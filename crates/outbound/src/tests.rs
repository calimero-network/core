//! Every gate of an outbound request, against a fake transport.

use std::collections::BTreeSet;

use calimero_bundle::BundleSecret;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use calimero_primitives::outbound::{OutboundMethod, OutboundRequest, OutboundResponse};

use super::*;

const VERIFY: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const HOST: &str = "challenges.cloudflare.com";
const SECRET: &str = "0x4AAA-very-secret";

/// mero-forms, whose manifest binds `TURNSTILE_SECRET` to Turnstile's `secret` body field,
/// for paths under `/turnstile/`.
fn forms() -> AppIdentity {
    AppIdentity {
        package: "com.calimero.mero-forms".to_owned(),
        signer: "did:key:zForms".to_owned(),
        secrets: BTreeMap::from([(
            "TURNSTILE_SECRET".to_owned(),
            BundleSecret::body_field(HOST, &["/turnstile/"], "secret"),
        )]),
        own: OwnSecrets::default(),
    }
}

/// An approval of exactly what `declared` declares, so a test of
/// another gate is not refused by this one.
fn approving(declared: &BTreeSet<String>) -> Approval {
    Approval {
        prefixes: declared.iter().cloned().collect(),
        reviewed: declared.iter().cloned().collect(),
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
        .prepare(
            &forms(),
            &declared(),
            Some(&approving(&declared())),
            request,
        )
        .expect_err("expected a refusal")
        .to_string()
}

#[test]
fn a_declared_request_reaches_the_host_with_its_secret_and_comes_back_redacted() {
    let fake = Arc::new(Fake::default());
    let run = service(&fake)
        .for_run(
            forms(),
            vec![VERIFY.to_owned()],
            Some(approving(&declared())),
            "ctx".to_owned(),
        )
        .expect("the app declares a URL");
    let request = post(VERIFY, "response=tok");
    let answer = run.send(&borsh::to_vec(&request).unwrap()).expect("sent");

    // The node attached the bound field; the app never wrote it.
    let sent = fake.0.lock().unwrap();
    assert_eq!(sent[0].0, VERIFY);
    assert_eq!(
        sent[0].1,
        format!("response=tok&secret={SECRET}").into_bytes()
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
    assert!(s
        .prepare(
            &forms(),
            &declared(),
            Some(&approving(&declared())),
            post(VERIFY, "")
        )
        .is_ok());
    assert!(s
        .prepare(
            &forms(),
            &declared(),
            Some(&approving(&declared())),
            post(&format!("{VERIFY}?x=1"), "")
        )
        .is_ok());
    assert!(s
        .prepare(
            &forms(),
            &declared(),
            Some(&approving(&declared())),
            post(&format!("{VERIFY}/v2"), "")
        )
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
    assert!(s.for_run(forms(), vec![], None, "ctx".to_owned()).is_none());
    for unusable in [
        "http://challenges.cloudflare.com/x",
        "https://challenges.cloudflare.com/x?key=1",
        "https://10.0.0.1/x",
    ] {
        assert!(s
            .for_run(forms(), vec![unusable.to_owned()], None, "ctx".to_owned())
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
            .prepare(
                &forms(),
                &BTreeSet::from([api.to_owned()]),
                Some(&approving(&BTreeSet::from([api.to_owned()]))),
                r.clone()
            )
            .is_ok());
    }
    // The secret is still bound to its own host: nothing is attached here.
    let p = s
        .prepare(
            &forms(),
            &BTreeSet::from([api.to_owned()]),
            Some(&approving(&BTreeSet::from([api.to_owned()]))),
            post("https://api.example.com/v1/x", "a=1"),
        )
        .unwrap();
    assert_eq!(&*p.body, b"a=1");
    assert!(p.secrets.is_empty());
}

#[test]
fn only_hosts_the_operator_lists_by_name_over_https_on_443() {
    let fake = Arc::new(Fake::default());
    let s = service(&fake);
    let refuse = |url: &str| {
        s.prepare(
            &forms(),
            &BTreeSet::from([url.to_owned()]),
            Some(&approving(&BTreeSet::from([url.to_owned()]))),
            post(url, ""),
        )
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
fn a_secret_goes_only_to_its_bound_host_and_only_for_its_own_app() {
    let fake = Arc::new(Fake::default());
    // The operator allows a second host, but the release binds the secret to Cloudflare
    // only, and the operator's binding names Cloudflare only.
    let s =
        OutboundService::with_parts(&[HOST, "api.example.com"], Some(Arc::new(OneSecret)), fake);
    let url = "https://api.example.com/hook";
    let p = s
        .prepare(
            &forms(),
            &BTreeSet::from([url.to_owned()]),
            Some(&approving(&BTreeSet::from([url.to_owned()]))),
            post(url, "x=1"),
        )
        .unwrap();
    assert!(p.secrets.is_empty() && &*p.body == b"x=1");

    // A release that binds it to the other host: the operator's binding refuses it.
    let rebound = AppIdentity {
        secrets: BTreeMap::from([(
            "TURNSTILE_SECRET".to_owned(),
            BundleSecret::body_field("api.example.com", &["/"], "secret"),
        )]),
        ..forms()
    };
    let leaked = s
        .prepare(
            &rebound,
            &BTreeSet::from([url.to_owned()]),
            Some(&approving(&BTreeSet::from([url.to_owned()]))),
            post(url, "x=1"),
        )
        .unwrap_err()
        .to_string();
    assert!(
        leaked.contains("other hosts than api.example.com"),
        "{leaked}"
    );

    // Another app binding the same name gets no value from the operator's binding, and
    // a required secret with no value is refused.
    let other = AppIdentity {
        package: "com.example.other".to_owned(),
        signer: "did:key:zOther".to_owned(),
        secrets: BTreeMap::from([(
            "TURNSTILE_SECRET".to_owned(),
            BundleSecret {
                required: true,
                ..BundleSecret::body_field(HOST, &["/turnstile/"], "secret")
            },
        )]),
        own: OwnSecrets::default(),
    };
    let stolen = s
        .prepare(
            &other,
            &declared(),
            Some(&approving(&declared())),
            post(VERIFY, "response=t"),
        )
        .unwrap_err()
        .to_string();
    assert!(stolen.contains("TURNSTILE_SECRET has no value"), "{stolen}");
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
}

#[test]
fn without_a_value_an_optional_secret_is_left_off_and_a_required_one_refused() {
    let fake = Arc::new(Fake::default());
    let s = OutboundService::with_parts(&[HOST], None, fake);
    let p = s
        .prepare(
            &forms(),
            &declared(),
            Some(&approving(&declared())),
            post(VERIFY, "ok"),
        )
        .unwrap();
    assert!(p.secrets.is_empty() && &*p.body == b"ok");
    let required = AppIdentity {
        secrets: BTreeMap::from([(
            "TURNSTILE_SECRET".to_owned(),
            BundleSecret {
                required: true,
                ..BundleSecret::body_field(HOST, &["/turnstile/"], "secret")
            },
        )]),
        ..forms()
    };
    let err = s
        .prepare(
            &required,
            &declared(),
            Some(&approving(&declared())),
            post(VERIFY, "ok"),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("has no value"), "{err}");
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
        sidecar: Some(crate::SidecarSettings {
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
            crate::EnvSecretSettings {
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
fn an_env_secret_is_for_its_packages_built_by_its_signers_only() {
    const VAR: &str = "CALIMERO_OUTBOUND_TEST_SIGNER_SCOPED";
    // No other test reads or writes this variable.
    std::env::set_var(VAR, SECRET);
    let settings = OutboundSettings {
        enabled: true,
        hosts: vec![HOST.to_owned()],
        env_secrets: [(
            "TURNSTILE_SECRET".to_owned(),
            crate::EnvSecretSettings {
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
            crate::EnvSecretSettings {
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

#[test]
fn no_debug_output_carries_a_secret() {
    const VAR: &str = "CALIMERO_OUTBOUND_TEST_DEBUG_REDACTED";
    // No other test reads or writes this variable.
    std::env::set_var(VAR, SECRET);
    let settings = OutboundSettings {
        enabled: true,
        hosts: vec![HOST.to_owned()],
        env_secrets: [(
            "TURNSTILE_SECRET".to_owned(),
            crate::EnvSecretSettings {
                env: VAR.to_owned(),
                packages: vec!["com.calimero.mero-forms".to_owned()],
                signers: vec!["did:key:zForms".to_owned()],
                hosts: vec![HOST.to_owned()],
            },
        )]
        .into(),
        ..OutboundSettings::default()
    };
    let service = OutboundService::from_settings(&settings).unwrap().unwrap();
    assert!(!format!("{service:?}").contains(SECRET));

    let prepared = service
        .prepare(
            &forms(),
            &declared(),
            Some(&approving(&declared())),
            post(VERIFY, ""),
        )
        .unwrap();
    assert_eq!(
        prepared.body.as_slice(),
        format!("secret={SECRET}").as_bytes()
    );
    assert!(!format!("{prepared:?}").contains(SECRET));
}

/// The approver is the third party: an operator who allows every host and a
/// manifest that declares the URL are not enough without its approval.
#[test]
fn a_namespace_that_approved_nothing_refuses_with_not_approved() {
    let s = service(&Arc::new(Fake::default()));
    let err = s
        .prepare(&forms(), &declared(), None, post(VERIFY, ""))
        .unwrap_err()
        .to_string();
    assert!(err.starts_with(NOT_APPROVED), "{err}");
    assert!(err.contains(VERIFY), "names the prefix: {err}");
}

#[test]
fn a_reviewed_prefix_left_out_is_not_approved_and_a_new_one_awaits_approval() {
    let s = service(&Arc::new(Fake::default()));
    let other = "https://challenges.cloudflare.com/v2/";
    let both = BTreeSet::from([VERIFY.to_owned(), other.to_owned()]);
    // The admin saw VERIFY and left it out (declined, or revoked later), and
    // never saw `other`, which a later version of the app added.
    let approval = Approval {
        prefixes: vec![],
        reviewed: vec![VERIFY.to_owned()],
    };
    let declined = s
        .prepare(&forms(), &both, Some(&approval), post(VERIFY, ""))
        .unwrap_err()
        .to_string();
    assert!(declined.starts_with(NOT_APPROVED), "{declined}");
    let new = s
        .prepare(
            &forms(),
            &both,
            Some(&approval),
            post(&format!("{other}x"), ""),
        )
        .unwrap_err()
        .to_string();
    assert!(new.starts_with(AWAITING_APPROVAL), "{new}");
    assert!(new.contains(other), "names the new prefix: {new}");
}

#[test]
fn an_approved_prefix_lets_its_urls_through_and_no_others() {
    let s = service(&Arc::new(Fake::default()));
    let other = "https://challenges.cloudflare.com/v2/";
    let both = BTreeSet::from([VERIFY.to_owned(), other.to_owned()]);
    // An upgrade added `other`: the approved prefix keeps working.
    let approval = Approval {
        prefixes: vec![VERIFY.to_owned()],
        reviewed: vec![VERIFY.to_owned()],
    };
    assert!(s
        .prepare(
            &forms(),
            &both,
            Some(&approval),
            post(&format!("{VERIFY}/v2"), "")
        )
        .is_ok());
    assert!(s
        .prepare(
            &forms(),
            &both,
            Some(&approval),
            post(&format!("{other}x"), "")
        )
        .unwrap_err()
        .to_string()
        .starts_with(AWAITING_APPROVAL));
}

/// An approval wider than the manifest does not widen what the app may reach:
/// all three of operator, manifest and approver must agree.
#[test]
fn an_approval_cannot_reach_past_the_manifest() {
    let s = service(&Arc::new(Fake::default()));
    let approval = Approval {
        prefixes: vec!["https://challenges.cloudflare.com/".to_owned()],
        reviewed: vec!["https://challenges.cloudflare.com/".to_owned()],
    };
    let err = s
        .prepare(
            &forms(),
            &declared(),
            Some(&approval),
            post("https://challenges.cloudflare.com/other", ""),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("under no URL prefix this app's manifest declares"),
        "{err}"
    );
}

/// A refusal reaches the app as an error it can read, not a trap.
#[test]
fn a_run_with_no_approval_still_gets_a_client_that_refuses() {
    let fake = Arc::new(Fake::default());
    let run = service(&fake)
        .for_run(forms(), vec![VERIFY.to_owned()], None, "ctx".to_owned())
        .expect("a client, so the refusal has somewhere to go");
    let err = run
        .send(&borsh::to_vec(&post(VERIFY, "")).unwrap())
        .unwrap_err();
    assert!(err.starts_with(NOT_APPROVED), "{err}");
    assert!(fake.0.lock().unwrap().is_empty(), "nothing was sent");
}

/// The node meters what a run sent (`calimero_app_outbound_requests_total`):
/// a request handed to the transport counts, a refused or malformed one never
/// left the node and does not.
#[test]
fn a_counted_run_counts_only_the_requests_it_sent() {
    let fake = Arc::new(Fake::default());
    let sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let approved = service(&fake)
        .for_run_counted(
            forms(),
            vec![VERIFY.to_owned()],
            Some(approving(&declared())),
            "ctx".to_owned(),
            Arc::clone(&sent),
            Arc::default(),
        )
        .unwrap();
    let _answer = approved
        .send(&borsh::to_vec(&post(VERIFY, "response=tok")).unwrap())
        .expect("sent");
    let _refused = approved
        .send(&borsh::to_vec(&post("https://elsewhere.example/", "")).unwrap())
        .unwrap_err();
    let _malformed = approved.send(b"not borsh").unwrap_err();
    let unapproved = service(&fake)
        .for_run_counted(
            forms(),
            vec![VERIFY.to_owned()],
            None,
            "ctx".to_owned(),
            Arc::clone(&sent),
            Arc::default(),
        )
        .unwrap();
    let _refused = unapproved
        .send(&borsh::to_vec(&post(VERIFY, "")).unwrap())
        .unwrap_err();
    assert_eq!(fake.0.lock().unwrap().len(), 1);
    assert_eq!(sent.load(std::sync::atomic::Ordering::Relaxed), 1);
}

mod merge {
    use calimero_primitives::outbound::OutboundSelection;

    use super::*;

    const A: &str = "https://a.example/v1/";
    const B: &str = "https://b.example/";

    fn owned(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn an_install_approving_the_manifest_approves_and_reviews_all_it_declares() {
        let merged =
            merge_approval(None, &owned(&[A, B]), &OutboundSelection::Manifest, false).unwrap();
        assert_eq!(merged.prefixes, owned(&[A, B]));
        assert_eq!(merged.reviewed, owned(&[A, B]));
    }

    #[test]
    fn approving_part_of_the_manifest_reviews_the_rest() {
        let merged = merge_approval(
            None,
            &owned(&[A, B]),
            &OutboundSelection::Prefixes(owned(&[A])),
            false,
        )
        .unwrap();
        assert_eq!(merged.prefixes, owned(&[A]));
        assert_eq!(merged.reviewed, owned(&[A, B]), "B was shown and left out");
    }

    #[test]
    fn an_upgrade_adds_and_never_drops_what_was_approved() {
        let existing = Approval {
            prefixes: owned(&[A]),
            reviewed: owned(&[A]),
        };
        let merged = merge_approval(
            Some(&existing),
            &owned(&[A, B]),
            &OutboundSelection::Prefixes(owned(&[B])),
            false,
        )
        .unwrap();
        assert_eq!(merged.prefixes, owned(&[A, B]));
    }

    #[test]
    fn replacing_revokes_and_keeps_the_revoked_prefix_reviewed() {
        let existing = Approval {
            prefixes: owned(&[A, B]),
            reviewed: owned(&[A, B]),
        };
        let merged = merge_approval(
            Some(&existing),
            &owned(&[A, B]),
            &OutboundSelection::Prefixes(owned(&[A])),
            true,
        )
        .unwrap();
        assert_eq!(merged.prefixes, owned(&[A]));
        assert_eq!(merged.reviewed, owned(&[A, B]));
    }

    #[test]
    fn a_selected_prefix_that_is_not_https_is_refused() {
        for bad in ["http://a.example/", "https://a.example/?q=1", "a.example"] {
            assert!(
                merge_approval(None, &[], &OutboundSelection::Prefixes(owned(&[bad])), true)
                    .is_err(),
                "{bad} was accepted"
            );
        }
    }

    #[test]
    fn more_prefixes_than_one_approval_holds_is_refused() {
        let many: Vec<String> = (0..=MAX_OUTBOUND_APPROVALS)
            .map(|i| format!("https://h{i}.example/"))
            .collect();
        assert!(merge_approval(None, &many, &OutboundSelection::Manifest, false).is_err());
    }
}

fn openai() -> AppIdentity {
    AppIdentity {
        secrets: BTreeMap::from([(
            "TURNSTILE_SECRET".to_owned(),
            BundleSecret::header(HOST, &["/turnstile/"], "authorization", Some("Bearer {}")),
        )]),
        ..forms()
    }
}

/// A header binding: the node sets the header in its format, on every request to the
/// host, and the app may neither set that header (in any case) nor name the secret.
#[test]
fn a_header_binding_is_attached_by_the_node_and_never_by_the_app() {
    let s = service(&Arc::new(Fake::default()));
    let prepare = |request| {
        s.prepare(
            &openai(),
            &declared(),
            Some(&approving(&declared())),
            request,
        )
    };
    let p = prepare(post(VERIFY, "{\"prompt\":\"hi\"}")).unwrap();
    assert_eq!(
        p.headers.last().map(|(n, v)| (n.as_str(), v.as_str())),
        Some(("authorization", format!("Bearer {SECRET}").as_str()))
    );
    assert_eq!(&*p.body, b"{\"prompt\":\"hi\"}");

    let mut own_header = post(VERIFY, "");
    own_header
        .headers
        .push(("AUTHORIZATION".to_owned(), "Bearer mine".to_owned()));
    let err = prepare(own_header).unwrap_err().to_string();
    assert!(
        err.contains("header AUTHORIZATION carries secret TURNSTILE_SECRET"),
        "{err}"
    );
}

/// A binding is attached only under its path prefixes, matched on whole segments: a
/// request to another path of the bound host carries no secret (and goes out as an
/// ordinary request), so the app cannot send the key to an endpoint it was not bound to.
#[test]
fn a_secret_is_attached_only_under_its_bound_paths_on_whole_segments() {
    let fake = Arc::new(Fake::default());
    let s = service(&fake);
    let stripe = AppIdentity {
        secrets: BTreeMap::from([(
            "TURNSTILE_SECRET".to_owned(),
            BundleSecret::header(HOST, &["/v1/charges"], "authorization", Some("Bearer {}")),
        )]),
        ..forms()
    };
    let host_wide = BTreeSet::from([format!("https://{HOST}/")]);
    let send = |path: &str| {
        s.prepare(
            &stripe,
            &host_wide,
            Some(&approving(&host_wide)),
            post(&format!("https://{HOST}{path}"), ""),
        )
        .unwrap()
    };
    let attached = |p: &Prepared| p.headers.iter().any(|(n, _)| n == "authorization");
    for under in [
        "/v1/charges",
        "/v1/charges/ch_1",
        "/v1/charges/ch_1/refunds?x=1",
    ] {
        let p = send(under);
        assert!(attached(&p) && p.secrets.len() == 1, "{under}");
    }
    for elsewhere in [
        "/v1/chargesX",
        "/v1/charge",
        "/v1/customers",
        "/",
        "/v1/charges/../customers",
        "/v1/charges/%2e%2e/customers",
        "/v1/charges%2Fx",
    ] {
        let p = send(elsewhere);
        assert!(!attached(&p) && p.secrets.is_empty(), "{elsewhere}");
    }
    // Off its paths the bound header is the app's own: nothing of the secret rides on it.
    let mut own = post(&format!("https://{HOST}/v1/customers"), "");
    own.headers
        .push(("Authorization".to_owned(), "Bearer mine".to_owned()));
    let p = s
        .prepare(&stripe, &host_wide, Some(&approving(&host_wide)), own)
        .unwrap();
    assert!(p.secrets.is_empty());
}

/// The retired placeholder is refused wherever it appears, never filled in, and the
/// refusal points at the guide.
#[test]
fn a_request_naming_a_secret_by_placeholder_is_refused_anywhere() {
    let s = service(&Arc::new(Fake::default()));
    let mut in_header = post(VERIFY, "");
    in_header
        .headers
        .push(("X-Key".to_owned(), "{{secret:TURNSTILE_SECRET}}".to_owned()));
    for request in [
        post(VERIFY, "secret={{secret:TURNSTILE_SECRET}}"),
        post(VERIFY, "{{secret:"),
        post(&format!("{VERIFY}?k={{{{secret:TURNSTILE_SECRET}}}}"), ""),
        post(
            &format!("{VERIFY}?k=%7B%7Bsecret:TURNSTILE_SECRET%7D%7D"),
            "",
        ),
        in_header,
    ] {
        let err = refusal(&s, request);
        assert!(err.starts_with(PLACEHOLDER_REFUSED), "{err}");
        assert!(err.contains(SECRETS_DOCS), "{err}");
    }
}

/// A body-field binding fills a form-urlencoded or JSON body's top-level field, and only
/// such a body; the app may not set the field itself, however it spells it.
#[test]
fn a_body_field_binding_fills_a_form_or_json_body_and_only_the_node_sets_it() {
    let s = service(&Arc::new(Fake::default()));
    let with_type = |content_type: &str, body: &str| OutboundRequest {
        headers: vec![("Content-Type".to_owned(), content_type.to_owned())],
        ..post(VERIFY, body)
    };
    let body_of = |request| {
        s.prepare(
            &forms(),
            &declared(),
            Some(&approving(&declared())),
            request,
        )
        .map(|p| String::from_utf8(p.body.to_vec()).unwrap())
        .map_err(|e| e.to_string())
    };
    let form = "application/x-www-form-urlencoded";
    assert_eq!(
        body_of(with_type(form, "response=t")).unwrap(),
        format!("response=t&secret={SECRET}")
    );
    assert_eq!(
        body_of(with_type(form, "")).unwrap(),
        format!("secret={SECRET}")
    );
    assert_eq!(
        body_of(with_type(
            "application/json; charset=utf-8",
            r#" {"response":"t"}"#
        ))
        .unwrap(),
        format!(r#" {{"secret":"{SECRET}","response":"t"}}"#)
    );
    assert_eq!(
        body_of(with_type("application/json", "")).unwrap(),
        format!(r#"{{"secret":"{SECRET}"}}"#)
    );
    for (content_type, body) in [
        (form, "secret=mine&response=t"),
        (form, "s%65cret=mine"),
        ("application/json", r#"{"secret":"mine"}"#),
        ("application/json", r#"{"secr\u0065t":"mine"}"#),
    ] {
        let err = body_of(with_type(content_type, body)).unwrap_err();
        assert!(err.contains("may not"), "{body}: {err}");
    }
    for (content_type, body) in [
        ("text/plain", "secret"),
        ("application/json", "[1]"),
        ("application/json", "{"),
    ] {
        assert!(body_of(with_type(content_type, body)).is_err(), "{body}");
    }
    let mut get = with_type(form, "");
    get.method = OutboundMethod::Get;
    assert!(body_of(get).unwrap_err().contains("GET has no body"));
}

/// An instance's own value for a bound name wins over the operator's and goes only where
/// its binding says; a release that binds nothing never uses one.
#[test]
fn an_instances_own_value_wins_for_its_bound_name_only() {
    let fake = Arc::new(Fake::default());
    let s = service(&fake);
    let own = OwnSecrets(BTreeMap::from([(
        "TURNSTILE_SECRET".to_owned(),
        Zeroizing::new("owners-own".to_owned()),
    )]));
    let prepare = |app: &AppIdentity| {
        s.prepare(
            app,
            &declared(),
            Some(&approving(&declared())),
            post(VERIFY, ""),
        )
    };
    let owned = AppIdentity {
        own: own.clone(),
        ..forms()
    };
    let p = prepare(&owned).unwrap();
    assert_eq!(&*p.body, b"secret=owners-own");
    // Without the instance's value: the operator's.
    let p = prepare(&forms()).unwrap();
    assert_eq!(&*p.body, format!("secret={SECRET}").as_bytes());
    // A release binding nothing attaches nothing.
    let p = prepare(&AppIdentity {
        secrets: BTreeMap::new(),
        own,
        ..forms()
    })
    .unwrap();
    assert!(p.body.is_empty() && p.secrets.is_empty());
    // Never in a debug print.
    assert!(!format!("{owned:?}").contains("owners-own"));
}

/// Every request a run hands the node lands in its instance's audit, sent or refused,
/// with the host, the path without its query, the name of the credential binding the
/// node attached (or refused) and the sizes: never the secret's value, a body, a query
/// string or a header.
#[test]
fn the_audit_records_sent_and_refused_requests_without_values() {
    let fake = Arc::new(Fake::default());
    let audit = Arc::new(OutboundAudit::default());
    let run_of = |app: AppIdentity, provider: Option<Arc<dyn SecretProvider>>| {
        OutboundService::with_parts(&[HOST], provider, fake.clone())
            .for_run_counted(
                app,
                vec![VERIFY.to_owned()],
                Some(approving(&declared())),
                "instance-1".to_owned(),
                Arc::default(),
                Arc::clone(&audit),
            )
            .expect("the app declares a URL")
    };
    let run = run_of(forms(), Some(Arc::new(OneSecret)));
    // Sent: the node attaches the bound body field.
    let body = "response=tok-private";
    let request = post(&format!("{VERIFY}?remoteip=10.1.2.3"), body);
    let _answer = run.send(&borsh::to_vec(&request).unwrap()).expect("sent");
    // Refused: the app sets the bound field itself.
    let own_field = post(VERIFY, "secret=guessed&response=tok-private");
    let _e = run
        .send(&borsh::to_vec(&own_field).unwrap())
        .expect_err("the node sets the bound field");
    // Refused: a required binding with no value on this node.
    let mut required = forms();
    for binding in required.secrets.values_mut() {
        binding.required = true;
    }
    let _e = run_of(required, None)
        .send(&borsh::to_vec(&post(VERIFY, body)).unwrap())
        .expect_err("no value for a required binding");
    // Refused: the retired placeholder, which names no binding.
    let named = post(VERIFY, "secret={{secret:NOT_BOUND}}");
    let _e = run
        .send(&borsh::to_vec(&named).unwrap())
        .expect_err("a placeholder");
    let elsewhere = post("https://challenges.cloudflare.com/other", "");
    let _e = run
        .send(&borsh::to_vec(&elsewhere).unwrap())
        .expect_err("not declared");
    let _e = run.send(b"\xff").expect_err("malformed");

    let entries = audit.entries("instance-1");
    assert_eq!(entries.len(), 6, "{entries:?}");
    let sent = &entries[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.host, HOST);
    assert_eq!(sent.path, "/turnstile/v0/siteverify");
    assert_eq!(sent.status, Some(200));
    assert_eq!(sent.failure, None);
    assert_eq!(sent.secrets, ["TURNSTILE_SECRET"]);
    assert_eq!(sent.request_bytes, body.len() as u64);
    assert!(sent.response_bytes > 0);
    for refused in &entries[1..3] {
        assert_eq!(refused.failure, Some(AuditFailure::Secret), "{refused:?}");
        assert_eq!(refused.secrets, ["TURNSTILE_SECRET"], "{refused:?}");
        assert_eq!(refused.status, None);
    }
    assert_eq!(entries[3].failure, Some(AuditFailure::Secret));
    assert!(entries[3].secrets.is_empty(), "{:?}", entries[3]);
    assert_eq!(entries[4].failure, Some(AuditFailure::Refused));
    assert_eq!(entries[4].path, "/other");
    assert!(entries[4].secrets.is_empty());
    assert_eq!(entries[5].failure, Some(AuditFailure::Malformed));

    let json = serde_json::to_string(&entries).unwrap();
    for leak in [
        SECRET,
        "guessed",
        "remoteip",
        "10.1.2.3",
        "tok-private",
        "NOT_BOUND",
        "Content-Type",
    ] {
        assert!(!json.contains(leak), "{leak} in {json}");
    }
    assert!(audit.entries("instance-2").is_empty());
}

#[test]
fn egress_off_refuses_every_request_whatever_was_approved_until_it_is_on_again() {
    use std::sync::atomic::AtomicBool;

    let fake = Arc::new(Fake::default());
    let run = service(&fake)
        .for_run(
            forms(),
            vec![VERIFY.to_owned()],
            Some(approving(&declared())),
            "ctx".to_owned(),
        )
        .expect("the app declares a URL");
    let off = Arc::new(AtomicBool::new(true));
    let switch = Arc::clone(&off);
    let audit = Arc::new(OutboundAudit::default());
    let gated = egress_gated(
        run,
        Arc::new(move || switch.load(Ordering::SeqCst).then_some("its organization")),
        Arc::clone(&audit),
        "ctx".to_owned(),
    );
    let request = borsh::to_vec(&post(VERIFY, "response=tok")).unwrap();

    let refused = gated.send(&request).expect_err("egress is off");
    assert!(
        refused.starts_with("EgressOff: its organization"),
        "{refused}"
    );
    assert_eq!(
        calimero_primitives::outbound::OutboundRefusal::of(&refused),
        Some(calimero_primitives::outbound::OutboundRefusal::EgressOff)
    );
    assert!(
        fake.0.lock().unwrap().is_empty(),
        "nothing reached the transport"
    );
    // The owner's audit names the refusal as egress off, with the endpoint.
    let entries = audit.entries("ctx");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].failure, Some(AuditFailure::EgressOff));
    assert_eq!(entries[0].host, HOST);
    assert_eq!(entries[0].path, "/turnstile/v0/siteverify");
    assert_eq!(entries[0].status, None);

    // Asked again for each request: on again, the approved request goes out.
    off.store(false, Ordering::SeqCst);
    let _answer = gated.send(&request).expect("egress is on");
    assert_eq!(fake.0.lock().unwrap().len(), 1);
}

/// A secret with bytes every encoding below changes: `/`, `+`, `=`, a quote, a
/// non-ASCII letter, and bytes whose base64 differs between the standard and
/// the URL-safe alphabet.
const AWKWARD: &str = "k3y/+=\"é\"?>~?>~";

/// The encodings of [`AWKWARD`] an upstream might echo, worked out outside this
/// crate (Python's `base64`, `bytes.hex`, `urllib.parse.quote`, `json.dumps`), so
/// the test does not check the code against itself.
const AWKWARD_ECHOES: &[(&str, &str)] = &[
    ("raw", AWKWARD),
    ("base64", "azN5Lys9IsOpIj8+fj8+fg=="),
    ("base64, no padding", "azN5Lys9IsOpIj8+fj8+fg"),
    ("base64url", "azN5Lys9IsOpIj8-fj8-fg=="),
    ("base64url, no padding", "azN5Lys9IsOpIj8-fj8-fg"),
    ("hex", "6b33792f2b3d22c3a9223f3e7e3f3e7e"),
    ("HEX", "6B33792F2B3D22C3A9223F3E7E3F3E7E"),
    ("percent", "k3y%2F%2B%3D%22%C3%A9%22%3F%3E~%3F%3E~"),
    ("percent, lower", "k3y%2f%2b%3d%22%c3%a9%22%3f%3e~%3f%3e~"),
    ("JSON", r#"k3y/+=\"é\"?>~?>~"#),
    ("JSON, ASCII only", r#"k3y/+=\"\u00e9\"?>~?>~"#),
    // Inside a longer value: HTTP Basic `user:` + secret, and one at each other
    // offset of base64's 3-byte groups.
    ("Basic credentials", "dXNlcjprM3kvKz0iw6kiPz5+Pz5+"),
    ("base64url, inside", "YWI6azN5Lys9IsOpIj8-fj8-fiE="),
    ("base64, inside", "eGszeS8rPSLDqSI/Pn4/Pn55eg=="),
];

/// [`AWKWARD`], for mero-forms, bound to Cloudflare.
#[derive(Debug)]
struct Awkward;

impl SecretProvider for Awkward {
    fn secret(&self, _app: &AppIdentity, name: &str) -> EyreResult<Option<Secret>> {
        Ok((name == "TURNSTILE_SECRET").then(|| Secret {
            value: Zeroizing::new(AWKWARD.to_owned()),
            hosts: vec![HOST.to_owned()],
        }))
    }
}

/// Answers with every echo of [`AWKWARD`], one per line, or fails with them.
#[derive(Debug)]
struct EchoesEncoded {
    fail: bool,
}

impl Transport for EchoesEncoded {
    fn send(&self, _request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        let lines: Vec<String> = AWKWARD_ECHOES
            .iter()
            .map(|(label, echo)| format!("{label}: <{echo}>"))
            .collect();
        if self.fail {
            bail!("upstream refused: {}", lines.join("; "));
        }
        Ok((400, lines.join("\n").into_bytes()))
    }
}

fn awkward_run(fail: bool) -> Arc<dyn OutboundHost> {
    OutboundService::with_parts(
        &[HOST],
        Some(Arc::new(Awkward)),
        Arc::new(EchoesEncoded { fail }),
    )
    .for_run(
        forms(),
        vec![VERIFY.to_owned()],
        Some(approving(&declared())),
        "ctx".to_owned(),
    )
    .expect("the app declares a URL")
}

/// Every echo of [`AWKWARD`] still in `text`, by label.
fn leaked(text: &str) -> Vec<&'static str> {
    AWKWARD_ECHOES
        .iter()
        .filter(|(_, echo)| text.contains(echo))
        .map(|(label, _)| *label)
        .collect()
}

#[test]
fn a_secret_echoed_in_a_common_encoding_is_redacted_from_the_response() {
    let request = post(VERIFY, "");
    let answer = awkward_run(false)
        .send(&borsh::to_vec(&request).unwrap())
        .expect("sent");
    let body = String::from_utf8(OutboundResponse::try_from_slice(&answer).unwrap().body).unwrap();
    assert_eq!(
        leaked(&body),
        Vec::<&str>::new(),
        "encodings of the secret reached the app:\n{body}"
    );
    assert_eq!(
        body.lines().filter(|l| l.contains("[redacted]")).count(),
        AWKWARD_ECHOES.len(),
        "each echo is redacted where it was:\n{body}"
    );
}

#[test]
fn a_failure_names_only_its_kind_never_the_transports_words() {
    // The transport's words can quote what it was sent; they reach neither the
    // guest nor the node's `outbound request failed` log line, which ships to
    // the operator.
    let request = post(VERIFY, "");
    let reason = awkward_run(true)
        .send(&borsh::to_vec(&request).unwrap())
        .expect_err("the transport failed");
    assert_eq!(reason, format!("the request to {HOST} failed: other"));
}

#[test]
fn a_secret_echoed_in_a_common_encoding_is_redacted_from_a_failure() {
    // The same reason goes to the guest and into the node's `outbound request
    // failed` log line.
    let request = post(VERIFY, "");
    let reason = awkward_run(true)
        .send(&borsh::to_vec(&request).unwrap())
        .expect_err("the transport failed");
    assert_eq!(
        leaked(&reason),
        Vec::<&str>::new(),
        "encodings of the secret reached the app and the log: {reason}"
    );
}

#[test]
fn the_audit_records_a_transport_failure_by_kind_never_its_words() {
    // An owner reads the audit: like the guest and the log, it learns only that
    // the request failed in transit, never what the transport said about it.
    let audit = Arc::new(OutboundAudit::default());
    let run = OutboundService::with_parts(
        &[HOST],
        Some(Arc::new(Awkward)),
        Arc::new(EchoesEncoded { fail: true }),
    )
    .for_run_counted(
        forms(),
        vec![VERIFY.to_owned()],
        Some(approving(&declared())),
        "ctx".to_owned(),
        Arc::default(),
        Arc::clone(&audit),
    )
    .expect("the app declares a URL");
    let request = post(VERIFY, "response=tok");
    let _e = run
        .send(&borsh::to_vec(&request).unwrap())
        .expect_err("the transport failed");

    let entries = audit.entries("ctx");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].failure, Some(AuditFailure::Failed));
    assert_eq!(entries[0].status, None);
    assert_eq!(
        entries[0].secrets,
        ["TURNSTILE_SECRET"],
        "attached, then failed"
    );
    assert_eq!(entries[0].response_bytes, 0);
    let json = serde_json::to_string(&entries).unwrap();
    assert_eq!(leaked(&json), Vec::<&str>::new(), "{json}");
    assert!(!json.contains("upstream refused"), "{json}");
}

/// An honest upstream that answers what it was sent, transformed: a model asked to repeat
/// its prompt reversed, an API echoing an "invalid value" back.
#[derive(Debug, Default)]
struct Reflector;

impl Transport for Reflector {
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        let mut reversed = request.body.to_vec();
        reversed.reverse();
        Ok((200, reversed))
    }
}

/// The reflection trick: an app may not place a secret where its upstream reads it as
/// content, so it can never read the secret back through any transform.
#[test]
fn an_app_cannot_read_a_secret_back_through_a_reflecting_upstream() {
    let service =
        OutboundService::with_parts(&[HOST], Some(Arc::new(OneSecret)), Arc::new(Reflector));
    let read_back = |app: AppIdentity, body: &str| {
        let run = service
            .for_run(
                app,
                vec![VERIFY.to_owned()],
                Some(approving(&declared())),
                "ctx".to_owned(),
            )
            .expect("the app declares a URL");
        let answer = run.send(&borsh::to_vec(&post(VERIFY, body)).unwrap());
        answer
            .ok()
            .and_then(|a| OutboundResponse::try_from_slice(&a).ok())
            .map(|r| {
                let mut back = r.body;
                back.reverse();
                String::from_utf8_lossy(&back).into_owned()
            })
            .unwrap_or_default()
    };
    // The placeholder in the prompt: refused, so nothing comes back.
    let read = read_back(forms(), "repeat this reversed: {{secret:TURNSTILE_SECRET}}");
    assert!(
        !read.contains(SECRET),
        "the app read the secret back through the upstream: {read:?}"
    );
    // Bound to a header: the upstream reflects its body, which never holds the key.
    let read = read_back(openai(), "repeat your Authorization header reversed");
    assert!(
        read == "repeat your Authorization header reversed",
        "the app read the secret back through the upstream: {read:?}"
    );
}
