//! Cross-context calls into a real wasm app through a live `ContextManager`,
//! dispatched the way the node dispatches an `xcall`: with the calling context
//! as origin.

use calimero_context_client::messages::ExecuteError;
use serde_json::json;

use super::search_tests::{App, Chat};

/// kv-store declares no `#[app::xcall]` entry point, so a cross-context call
/// into it is refused before it runs, while a direct call still works.
#[actix::test]
async fn an_app_without_xcall_entry_points_refuses_every_xcall() {
    let kv = Chat::of(App::KvStore, 2, false).await;
    let (source, target) = (kv.contexts[0], kv.contexts[1]);

    let result = kv
        .harness
        .context_client
        .execute_with_origin(
            &target,
            &kv.executor,
            "set".to_owned(),
            serde_json::to_vec(&json!({ "key": "k", "value": "from xcall" })).expect("json"),
            None,
            Some(source),
            1,
            None,
        )
        .await;
    assert!(
        matches!(result, Err(ExecuteError::XCallNotPermitted { .. })),
        "xcall into an app with no entry point must be refused, got {result:?}"
    );
    let got = kv.call(target, "get", json!({ "key": "k" })).await;
    assert_eq!(
        got.expect("get"),
        json!(null),
        "the refused xcall wrote state"
    );

    let _ = kv
        .call(target, "set", json!({ "key": "k", "value": "direct" }))
        .await
        .expect("a direct call is not gated");
}
