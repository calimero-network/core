//! The connector inside a real component on the real engine, NEAR AI answered by the harness.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use calimero_app_build::build_in_repo_cache_with;
use calimero_sdk::testing::{Failure, HttpRequest, HttpResponse, Org, TestApp};
use serde_json::{json, Value};

const T0: u64 = 1_700_000_000_000;
const HOST: &str = "cloud-api.near.ai";

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let connector = Path::new(env!("CARGO_MANIFEST_DIR"));
        // The guest compiles this crate from its path: hash it, or an edit here goes unseen.
        build_in_repo_cache_with(&connector.join("tests/guest"), &[connector])
            .unwrap_or_else(|e| panic!("build the guest: {e}"))
            .app
            .component
    })
}

/// The guest with NEAR AI answering `status` and `body`, and the requests it saw.
fn guest(status: u16, body: Value) -> (TestApp, Arc<Mutex<Vec<HttpRequest>>>) {
    let mut t = TestApp::new(component(), Org::new(), T0);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    t.mock_http(HOST, move |req| {
        log.lock().expect("log").push(req.clone());
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.to_string().into_bytes(),
        }
    });
    (t, seen)
}

fn prompt() -> Value {
    json!({
        "model": "Qwen/Qwen3.8-27B",
        "max_tokens": 64,
        "messages": [{ "role": "user", "content": "Capital of France?" }]
    })
}

#[test]
fn an_action_posts_the_prompt_with_no_key_and_returns_the_answer() {
    let (mut t, seen) = guest(
        200,
        json!({
            "model": "qwen/qwen3.8-27b-fp8",
            "choices": [{
                "message": { "role": "assistant", "content": "Paris.", "reasoning_content": "Hm." },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 2, "total_tokens": 14 }
        }),
    );
    let got = t
        .as_("owner")
        .action("ask", json!({ "prompt": prompt() }))
        .expect("an answer");
    assert_eq!(
        got,
        json!({
            "message": { "role": "assistant", "content": "Paris." },
            "finish_reason": "stop",
            "usage": { "prompt_tokens": 12, "completion_tokens": 2, "total_tokens": 14 }
        })
    );
    let seen = seen.lock().expect("log");
    assert_eq!(seen.len(), 1);
    let req = &seen[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.url, "https://cloud-api.near.ai/v1/chat/completions");
    assert_eq!(
        req.headers,
        [("content-type".to_owned(), "application/json".to_owned())]
    );
    let body: Value = serde_json::from_slice(&req.body).expect("a JSON body");
    assert_eq!(body, prompt());
}

#[test]
fn near_ais_error_reaches_the_app_as_its_typed_code() {
    let (mut t, _) = guest(
        400,
        json!({ "error": {
            "message": "temperature is not supported by this model",
            "type": "BadRequestError", "param": "temperature", "code": 400
        } }),
    );
    let got = t.as_("owner").action("ask", json!({ "prompt": prompt() }));
    assert_eq!(
        got,
        Err(Failure::App {
            code: "NearAiStatus".into(),
            fields: json!({ "status": 400, "message": "temperature is not supported by this model" }),
        })
    );
}

#[test]
fn a_request_the_node_cannot_send_is_not_sent_with_its_reason() {
    let mut t = TestApp::new(component(), Org::new(), T0);
    let got = t.as_("owner").action("ask", json!({ "prompt": prompt() }));
    let Err(Failure::App { code, fields }) = got else {
        panic!("expected an app error, got {got:?}");
    };
    assert_eq!(code, "NearAiNotSent");
    let reason = fields["reason"].as_str().expect("a reason");
    assert!(reason.contains(HOST), "{reason}");
}
