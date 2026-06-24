//! Wire-contract golden fixtures (the fast, Docker-free canary).
//!
//! Each committed `fixtures/wire/<area>/<name>.json` is the canonical JSON of an
//! HTTP request/response DTO. The test deserializes it into the Rust type and
//! re-serializes: a field rename, removal, or retype makes the round-trip diverge
//! and fails here, with a diff naming the change. SDKs mirror these types by hand,
//! so this is the first signal that a wire change needs a matching SDK update.
//!
//! Regenerate after an intended change:
//!   UPDATE_FIXTURES=1 cargo test -p calimero-server-primitives --test wire_fixtures

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponseData, ReparentGroupApiRequest,
    ReparentGroupApiResponse,
};
use calimero_server_primitives::jsonrpc::{ExecutionRequest, ExecutionResponse};

fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/wire")
        .join(rel)
}

fn check<T: DeserializeOwned + Serialize>(rel: &str) -> Result<(), String> {
    let path = fixture_path(rel);
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("{rel}: cannot read fixture: {e}"))?;

    // Deserialize into the DTO — a removed/renamed required field fails right here.
    let typed: T = serde_json::from_str(&raw)
        .map_err(|e| format!("{rel}: does not deserialize into {}: {e}", type_name::<T>()))?;
    let canonical = serde_json::to_value(&typed).map_err(|e| format!("{rel}: {e}"))?;

    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        let pretty = serde_json::to_string_pretty(&canonical).map_err(|e| e.to_string())?;
        std::fs::write(&path, format!("{pretty}\n")).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let committed: Value =
        serde_json::from_str(&raw).map_err(|e| format!("{rel}: invalid JSON: {e}"))?;
    if canonical != committed {
        return Err(format!(
            "{rel}: wire shape drifted.\n--- committed ---\n{}\n--- {} now serializes ---\n{}\n\
             (after an intended change run: UPDATE_FIXTURES=1 cargo test -p calimero-server-primitives --test wire_fixtures, \
             then update the matching SDK type)",
            serde_json::to_string_pretty(&committed).unwrap_or_default(),
            type_name::<T>(),
            serde_json::to_string_pretty(&canonical).unwrap_or_default(),
        ));
    }
    Ok(())
}

fn type_name<T>() -> &'static str {
    std::any::type_name::<T>()
}

macro_rules! wire_fixtures {
    ($($test:ident: $ty:ty => $rel:literal),* $(,)?) => {
        $(
            #[test]
            fn $test() {
                if let Err(e) = check::<$ty>($rel) {
                    panic!("{e}");
                }
            }
        )*
    };
}

// Scope: the endpoints that drifted (mero-js #51/#53) + jsonrpc. Adding a fixture
// is one row here plus the committed JSON; expanding to the full DTO surface and
// the auth-crate DTOs is mechanical.
wire_fixtures! {
    create_context_req: CreateContextRequest => "contexts/create_context.req.json",
    create_context_res: CreateContextResponseData => "contexts/create_context.res.json",
    reparent_req: ReparentGroupApiRequest => "groups/reparent.req.json",
    reparent_res: ReparentGroupApiResponse => "groups/reparent.res.json",
    execute_req: ExecutionRequest => "jsonrpc/execute.req.json",
    execute_res: ExecutionResponse => "jsonrpc/execute.res.json",
}
