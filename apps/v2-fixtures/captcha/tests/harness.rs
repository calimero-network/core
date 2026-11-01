//! The captcha service on the real engine: challenges, single-use verification by apps.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::build_component;
use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::json;

fn component() -> &'static [u8] {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("captcha-guest");
        build_component(dir, &target)
            .unwrap_or_else(|e| panic!("build captcha: {e}"))
            .component
    })
}

fn app() -> TestApp {
    TestApp::new(component(), Org::new().members(["bob"]), 1_700_000_000_000)
}

#[test]
fn a_challenge_is_verified_once_by_an_app() {
    let mut t = app();
    let puzzle = t.as_("bob").call("challenge", json!({})).unwrap();
    let id = puzzle["id"].as_str().unwrap().to_owned();
    assert!(puzzle["question"].as_str().unwrap().starts_with("What is "));
    assert!(puzzle.get("answer").is_none(), "the answer never leaves");
    let answer = t
        .as_("owner")
        .view("answerOf", json!({ "id": id }))
        .unwrap();
    assert_eq!(
        t.as_("bob").view("answerOf", json!({ "id": id })).unwrap(),
        json!(null)
    );
    assert_eq!(
        t.as_("bob").view("status", json!({ "id": id })).unwrap(),
        json!("open")
    );

    // A person cannot spend it; an app can, once.
    let e = t
        .as_("bob")
        .call("verify", json!({ "id": id, "answer": answer }))
        .unwrap_err();
    assert!(
        matches!(e, Failure::App { ref code, .. } if code == "AppsOnly"),
        "{e:?}"
    );
    assert_eq!(
        t.deliver("forms", "verify", json!({ "id": id, "answer": answer })),
        Ok(json!(true))
    );
    assert_eq!(
        t.deliver("forms", "verify", json!({ "id": id, "answer": answer })),
        Ok(json!(false))
    );
    assert_eq!(
        t.as_("bob").view("status", json!({ "id": id })).unwrap(),
        json!("spent")
    );

    // A wrong answer spends it too, so it cannot be guessed at.
    let other = t.as_("bob").call("challenge", json!({})).unwrap()["id"].clone();
    assert_eq!(
        t.deliver("forms", "verify", json!({ "id": other, "answer": "-1" })),
        Ok(json!(false))
    );
    assert_eq!(
        t.as_("bob").view("status", json!({ "id": other })).unwrap(),
        json!("spent")
    );
    assert_eq!(
        t.deliver("forms", "verify", json!({ "id": "nope", "answer": "1" })),
        Ok(json!(false))
    );
}
