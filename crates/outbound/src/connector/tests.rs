//! A connector builds exactly its operation's request, goes through every gate
//! of a raw request, and returns only its declared fields; an app declaring one
//! cannot reach the connector's host any other way.

use std::sync::Mutex;

use zeroize::Zeroizing;

use super::*;
use crate::{OwnSecrets, Prepared, Secret, SecretProvider, Transport};

const STRIPE_KEY: &str = "sk_test_51Owner";
const OPENAI_KEY: &str = "sk-proj-owner";
const INTENTS: &str = "https://api.stripe.com/v1/payment_intents";

fn shop() -> AppIdentity {
    AppIdentity {
        package: "com.example.shop".to_owned(),
        signer: "did:key:zShop".to_owned(),
        secrets: None,
        own: OwnSecrets::default(),
    }
}

/// The operator's keys: Stripe's bound to api.stripe.com, OpenAI's to
/// api.openai.com, for the shop only.
#[derive(Debug)]
struct Keys;

impl SecretProvider for Keys {
    fn secret(&self, app: &AppIdentity, name: &str) -> EyreResult<Option<Secret>> {
        let (value, host) = match name {
            "STRIPE_SECRET_KEY" => (STRIPE_KEY, "api.stripe.com"),
            "OPENAI_API_KEY" => (OPENAI_KEY, "api.openai.com"),
            _ => return Ok(None),
        };
        Ok((app.package == "com.example.shop").then(|| Secret {
            value: Zeroizing::new(value.to_owned()),
            hosts: vec![host.to_owned()],
        }))
    }
}

/// What the upstream was sent: method, URL, headers, body.
type Sent = (OutboundMethod, String, Vec<(String, String)>, String);

/// An in-process upstream: records each request, answers `answer`.
#[derive(Debug)]
struct Upstream {
    sent: Mutex<Vec<Sent>>,
    answer: (u16, String),
}

impl Upstream {
    fn answering(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::default(),
            answer: (status, body.to_owned()),
        })
    }

    fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

impl Transport for Upstream {
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        self.sent.lock().unwrap().push((
            request.method,
            request.url.to_string(),
            request
                .headers
                .iter()
                .map(|(n, v)| (n.clone(), v.to_string()))
                .collect(),
            String::from_utf8(request.body.to_vec()).unwrap(),
        ));
        Ok((self.answer.0, self.answer.1.clone().into_bytes()))
    }
}

fn service(upstream: &Arc<Upstream>) -> Arc<OutboundService> {
    OutboundService::with_parts(
        &["api.stripe.com", "api.openai.com"],
        Some(Arc::new(Keys)),
        upstream.clone(),
    )
}

fn approving(prefixes: &[&str]) -> Approval {
    let prefixes: Vec<String> = prefixes.iter().map(|p| (*p).to_owned()).collect();
    Approval {
        reviewed: prefixes.clone(),
        prefixes,
    }
}

fn grant(name: &str, operations: &[&str], limits: &[(&str, &[&str])]) -> ConnectorGrant {
    ConnectorGrant {
        name: name.to_owned(),
        operations: operations.iter().map(|o| (*o).to_owned()).collect(),
        limits: limits
            .iter()
            .map(|(l, values)| {
                (
                    (*l).to_owned(),
                    values.iter().map(|v| (*v).to_owned()).collect(),
                )
            })
            .collect(),
    }
}

fn stripe_grant() -> ConnectorGrant {
    grant(
        "stripe",
        &["createPaymentIntent", "retrievePaymentIntent"],
        &[("metadata", &["order_id"])],
    )
}

/// The clients of a shop run declaring `outbound` and `connectors`, with every
/// connector prefix and `outbound` approved.
fn run(upstream: &Arc<Upstream>, outbound: &[&str], connectors: Vec<ConnectorGrant>) -> RunClients {
    let mut approved: Vec<String> = outbound.iter().map(|p| (*p).to_owned()).collect();
    for g in &connectors {
        approved.extend(connector(&g.name).unwrap().prefixes(g));
    }
    let approved: Vec<&str> = approved.iter().map(String::as_str).collect();
    service(upstream).for_run_with_connectors(
        shop(),
        outbound.iter().map(|p| (*p).to_owned()).collect(),
        connectors,
        Some(approving(&approved)),
        "ctx".to_owned(),
        Arc::default(),
    )
}

fn raw(url: &str, body: &str) -> Vec<u8> {
    borsh::to_vec(&OutboundRequest {
        method: OutboundMethod::Post,
        url: url.to_owned(),
        headers: vec![(
            "authorization".to_owned(),
            "Bearer {{secret:STRIPE_SECRET_KEY}}".to_owned(),
        )],
        body: body.as_bytes().to_vec(),
    })
    .unwrap()
}

fn call(clients: &RunClients, name: &str, operation: &str, input: &str) -> Result<Value, String> {
    let out = clients
        .connectors
        .as_ref()
        .expect("a connector client")
        .call(name, operation, input.as_bytes())?;
    Ok(serde_json::from_slice(&out).unwrap())
}

/// The problem (#5349): with the key bound to its host, a raw request reaches
/// any path the key allows, here a refund the owner never approved.
#[test]
fn without_a_connector_a_bound_stripe_key_reaches_any_stripe_path() {
    let upstream = Upstream::answering(200, r#"{"id":"re_1"}"#);
    let clients = run(&upstream, &["https://api.stripe.com/v1/"], vec![]);
    let _answer = clients
        .http
        .as_ref()
        .unwrap()
        .send(&raw(
            "https://api.stripe.com/v1/refunds",
            "payment_intent=pi_1",
        ))
        .expect("sent");
    let sent = upstream.sent();
    assert_eq!(sent[0].1, "https://api.stripe.com/v1/refunds");
    assert!(sent[0]
        .2
        .contains(&("authorization".to_owned(), format!("Bearer {STRIPE_KEY}"))));
}

#[test]
fn a_declared_connector_is_the_only_way_to_its_host() {
    let upstream = Upstream::answering(200, r#"{"id":"re_1"}"#);
    let clients = run(
        &upstream,
        &["https://api.stripe.com/v1/", "https://api.openai.com/v1/"],
        vec![stripe_grant()],
    );
    let http = clients.http.as_ref().expect("a raw client for other hosts");
    for url in [
        "https://api.stripe.com/v1/refunds",
        INTENTS,
        "https://API.Stripe.com/v1/refunds",
        "https://api.stripe.com./v1/refunds",
    ] {
        let err = http.send(&raw(url, "")).unwrap_err();
        assert!(
            err.contains("only through the stripe connector"),
            "{url}: {err}"
        );
    }
    assert!(upstream.sent().is_empty(), "nothing reached Stripe");
    // Another host the release declares is still reached raw.
    let models = OutboundRequest {
        method: OutboundMethod::Get,
        url: "https://api.openai.com/v1/models".to_owned(),
        headers: vec![(
            "authorization".to_owned(),
            "Bearer {{secret:OPENAI_API_KEY}}".to_owned(),
        )],
        body: vec![],
    };
    let _answer = http
        .send(&borsh::to_vec(&models).unwrap())
        .expect("another host is unaffected");
}

#[test]
fn create_payment_intent_sends_exactly_the_pinned_request() {
    let upstream = Upstream::answering(200, r#"{"id":"pi_1","status":"requires_payment_method"}"#);
    let clients = run(&upstream, &[], vec![stripe_grant()]);
    assert!(clients.http.is_none(), "no raw prefix, no raw client");
    let _out = call(
        &clients,
        "stripe",
        "createPaymentIntent",
        r#"{"amount":2000,"currency":"usd","customer":"cus_9s6XKzkNRiz8i3","metadata":{"order_id":"A-17"}}"#,
    )
    .unwrap();
    assert_eq!(
        upstream.sent(),
        vec![(
            OutboundMethod::Post,
            INTENTS.to_owned(),
            vec![
                ("stripe-version".to_owned(), "2024-06-20".to_owned()),
                ("authorization".to_owned(), format!("Bearer {STRIPE_KEY}")),
                (
                    "content-type".to_owned(),
                    "application/x-www-form-urlencoded".to_owned()
                ),
            ],
            "amount=2000&currency=usd&customer=cus_9s6XKzkNRiz8i3&metadata%5Border_id%5D=A-17"
                .to_owned(),
        )]
    );
}

#[test]
fn retrieve_payment_intent_puts_only_an_id_in_the_path() {
    let upstream = Upstream::answering(200, r#"{"id":"pi_3MtwBw"}"#);
    let clients = run(&upstream, &[], vec![stripe_grant()]);
    let _out = call(
        &clients,
        "stripe",
        "retrievePaymentIntent",
        r#"{"id":"pi_3MtwBw"}"#,
    )
    .unwrap();
    let sent = upstream.sent();
    assert_eq!(sent[0].0, OutboundMethod::Get);
    assert_eq!(sent[0].1, format!("{INTENTS}/pi_3MtwBw"));
    assert_eq!(sent[0].3, "");
    for id in [
        "pi_1/../../refunds",
        "pi_1?expand[]=customer",
        "re_1",
        "pi_",
        "pi_1%2F..",
    ] {
        let err = call(
            &clients,
            "stripe",
            "retrievePaymentIntent",
            &serde_json::json!({ "id": id }).to_string(),
        )
        .unwrap_err();
        assert!(err.contains("an id starting"), "{id}: {err}");
    }
    assert_eq!(upstream.sent().len(), 1);
}

#[test]
fn undeclared_operations_and_fields_are_refused_before_anything_is_sent() {
    let upstream = Upstream::answering(200, "{}");
    let clients = run(
        &upstream,
        &[],
        vec![grant(
            "stripe",
            &["createPaymentIntent"],
            &[("metadata", &["order_id"])],
        )],
    );
    let refused = |name: &str, operation: &str, input: &str| {
        call(&clients, name, operation, input).unwrap_err()
    };
    // An operation the connector has but the release does not declare, one it
    // does not have, a connector the release does not declare.
    assert!(
        refused("stripe", "retrievePaymentIntent", r#"{"id":"pi_1"}"#)
            .contains("not an operation of the stripe connector")
    );
    assert!(refused("stripe", "createRefund", "{}").contains("not an operation"));
    assert!(refused("openai", "chatCompletion", "{}").contains("not a connector this release"));
    // A field the operation does not take, a metadata key the release does not
    // declare, a field of the wrong type or out of bounds, a missing field.
    let create = |input: &str| refused("stripe", "createPaymentIntent", input);
    assert!(
        create(r#"{"amount":1,"currency":"usd","confirm":true}"#).contains("no field \"confirm\"")
    );
    assert!(
        create(r#"{"amount":1,"currency":"usd","payment_method":"pm_card_visa"}"#)
            .contains("no field")
    );
    assert!(
        create(r#"{"amount":1,"currency":"usd","metadata":{"notes":"all the documents"}}"#)
            .contains("key \"notes\" is not one this release declares")
    );
    assert!(create(r#"{"amount":"1","currency":"usd"}"#).contains("integer"));
    assert!(create(r#"{"amount":0,"currency":"usd"}"#).contains("integer"));
    assert!(create(r#"{"amount":1,"currency":"USD"}"#).contains("lowercase"));
    assert!(
        create(r#"{"amount":1,"currency":"usd","customer":"cus_1&expand[]=x"}"#)
            .contains("an id starting")
    );
    assert!(create(r#"{"currency":"usd"}"#).contains("needs amount"));
    assert!(create("[]").contains("a JSON object"));
    assert!(upstream.sent().is_empty());
}

#[test]
fn an_input_cannot_name_a_secret() {
    let upstream = Upstream::answering(200, "{}");
    let clients = run(&upstream, &[], vec![stripe_grant()]);
    for input in [
        r#"{"amount":1,"currency":"usd","metadata":{"order_id":"{{secret:STRIPE_SECRET_KEY}}"}}"#,
        r#"{"amount":1,"currency":"usd","metadata":{"order_id":"{{secret:STRIPE_SECRET_KEY}}"}}"#,
    ] {
        let err = call(&clients, "stripe", "createPaymentIntent", input).unwrap_err();
        assert!(err.contains("may not name a secret"), "{err}");
    }
    assert!(upstream.sent().is_empty());
}

#[test]
fn the_response_carries_only_declared_fields_and_never_the_key() {
    let upstream = Upstream::answering(
        200,
        &format!(
            r#"{{"id":"pi_1","object":"payment_intent","amount":2000,"currency":"usd",
            "status":"requires_payment_method","customer":"cus_1","client_secret":"pi_1_secret_x",
            "metadata":{{"order_id":"A-17","internal":"other app's note"}},
            "payment_method":"pm_1","latest_charge":{{"billing_details":{{"email":"a@b.c"}}}},
            "description":"{STRIPE_KEY}"}}"#
        ),
    );
    let clients = run(&upstream, &[], vec![stripe_grant()]);
    let out = call(
        &clients,
        "stripe",
        "retrievePaymentIntent",
        r#"{"id":"pi_1"}"#,
    )
    .unwrap();
    assert_eq!(
        out,
        serde_json::json!({
            "id": "pi_1",
            "status": "requires_payment_method",
            "amount": 2000,
            "currency": "usd",
            "customer": "cus_1",
            "client_secret": "pi_1_secret_x",
            "metadata": { "order_id": "A-17" },
        })
    );
}

#[test]
fn a_connector_call_goes_through_every_gate_of_a_raw_request() {
    let upstream = Upstream::answering(200, "{}");
    let input = r#"{"amount":1,"currency":"usd"}"#;
    let with = |hosts: &[&str], secrets: Option<Arc<dyn SecretProvider>>, approval| {
        OutboundService::with_parts(hosts, secrets, upstream.clone()).for_run_with_connectors(
            shop(),
            vec![],
            vec![stripe_grant()],
            approval,
            "ctx".to_owned(),
            Arc::default(),
        )
    };
    let approved = || Some(approving(&[INTENTS, &format!("{INTENTS}/")]));
    // The operator does not allow the host.
    let err = call(
        &with(&["api.openai.com"], Some(Arc::new(Keys)), approved()),
        "stripe",
        "createPaymentIntent",
        input,
    )
    .unwrap_err();
    assert!(
        err.contains("not a host this node's operator allows"),
        "{err}"
    );
    // Nobody approved it.
    let err = call(
        &with(&["api.stripe.com"], Some(Arc::new(Keys)), None),
        "stripe",
        "createPaymentIntent",
        input,
    )
    .unwrap_err();
    assert!(err.starts_with(crate::NOT_APPROVED), "{err}");
    // No key for it.
    let err = call(
        &with(&["api.stripe.com"], None, approved()),
        "stripe",
        "createPaymentIntent",
        input,
    )
    .unwrap_err();
    assert!(
        err.contains("no value for secret STRIPE_SECRET_KEY"),
        "{err}"
    );
    assert!(upstream.sent().is_empty());

    // A release that declares its raw secrets still gets the connector's key,
    // only to the connector's host, and the call counts as a request.
    let sent = Arc::new(AtomicU64::new(0));
    let mut declaring = shop();
    declaring.secrets = Some(BTreeMap::new());
    let clients = service(&upstream).for_run_with_connectors(
        declaring,
        vec![],
        vec![stripe_grant()],
        approved(),
        "ctx".to_owned(),
        Arc::clone(&sent),
    );
    let _out = call(&clients, "stripe", "createPaymentIntent", input).unwrap();
    assert_eq!(sent.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn an_upstream_refusal_returns_its_status_and_code_only() {
    let upstream = Upstream::answering(
        402,
        r#"{"error":{"code":"card_declined","type":"card_error","message":"Your card cus_secret_note was declined."}}"#,
    );
    let clients = run(&upstream, &[], vec![stripe_grant()]);
    let err = call(
        &clients,
        "stripe",
        "retrievePaymentIntent",
        r#"{"id":"pi_1"}"#,
    )
    .unwrap_err();
    assert_eq!(err, "stripe answered 402: card_declined");
    let upstream = Upstream::answering(500, "<html>oops</html>");
    let clients = run(&upstream, &[], vec![stripe_grant()]);
    let err = call(
        &clients,
        "stripe",
        "retrievePaymentIntent",
        r#"{"id":"pi_1"}"#,
    )
    .unwrap_err();
    assert_eq!(err, "stripe answered 500");
}

mod openai {
    use super::*;

    fn openai_grant(models: &[&str]) -> ConnectorGrant {
        grant("openai", &["chatCompletion"], &[("model", models)])
    }

    #[test]
    fn a_chat_completion_sends_the_pinned_body_and_returns_content_and_usage_only() {
        let upstream = Upstream::answering(
            200,
            r#"{"id":"chatcmpl-1","object":"chat.completion","model":"gpt-4o-mini",
            "choices":[{"index":0,"message":{"role":"assistant","content":"Hi.","refusal":null},
            "logprobs":null,"finish_reason":"stop"}],
            "usage":{"prompt_tokens":9,"completion_tokens":2,"total_tokens":11,
            "prompt_tokens_details":{"cached_tokens":0}},"system_fingerprint":"fp_1"}"#,
        );
        let clients = run(&upstream, &[], vec![openai_grant(&["gpt-4o-mini"])]);
        let out = call(
            &clients,
            "openai",
            "chatCompletion",
            r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hello"}],"max_tokens":16}"#,
        )
        .unwrap();
        assert_eq!(
            out,
            serde_json::json!({
                "content": "Hi.",
                "usage": { "prompt_tokens": 9, "completion_tokens": 2, "total_tokens": 11 },
            })
        );
        let sent = upstream.sent();
        assert_eq!(sent[0].1, "https://api.openai.com/v1/chat/completions");
        assert_eq!(
            sent[0].2,
            vec![
                ("authorization".to_owned(), format!("Bearer {OPENAI_KEY}")),
                ("content-type".to_owned(), "application/json".to_owned()),
            ]
        );
        let body: Value = serde_json::from_str(&sent[0].3).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "model": "gpt-4o-mini",
                "messages": [{ "role": "user", "content": "Hello" }],
                "max_tokens": 16,
            })
        );
    }

    #[test]
    fn only_models_the_owner_allows_and_only_plain_messages() {
        let upstream = Upstream::answering(200, "{}");
        let refused = |models: &[&str], input: &str| {
            call(
                &run(&upstream, &[], vec![openai_grant(models)]),
                "openai",
                "chatCompletion",
                input,
            )
            .unwrap_err()
        };
        let hello = r#"[{"role":"user","content":"Hello"}]"#;
        assert!(refused(
            &["gpt-4o-mini"],
            &format!(r#"{{"model":"o1","messages":{hello}}}"#)
        )
        .contains("not a model allowed"));
        assert!(refused(
            &[],
            &format!(r#"{{"model":"gpt-4o-mini","messages":{hello}}}"#)
        )
        .contains("no model is allowed"));
        let ok = &["gpt-4o-mini"][..];
        for messages in [
            r#"[]"#,
            r#"[{"role":"tool","content":"x"}]"#,
            r#"[{"role":"user","content":"x","name":"y"}]"#,
            r#"[{"role":"user","content":[{"type":"image_url"}]}]"#,
        ] {
            let err = refused(
                ok,
                &format!(r#"{{"model":"gpt-4o-mini","messages":{messages}}}"#),
            );
            assert!(err.contains("messages"), "{messages}: {err}");
        }
        for extra in [r#""tools":[]"#, r#""stream":true"#, r#""n":5"#] {
            let err = refused(
                ok,
                &format!(r#"{{"model":"gpt-4o-mini","messages":{hello},{extra}}}"#),
            );
            assert!(err.contains("takes no field"), "{extra}: {err}");
        }
        assert!(upstream.sent().is_empty());
    }
}
