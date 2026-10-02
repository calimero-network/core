//! How the context client answers a root-state merge for a module that
//! exports no `__calimero_merge_root_state`.

use calimero_context_client::messages::MethodNotExported;
use calimero_primitives::context::GroupMemberRole;
use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};
use calimero_storage::merge::{MergeRootStateRequest, MergeRootStateResponse};

use super::returns_error;
use super::state_write_gate_tests::{fixture, LocalRole};

/// A module without the export holds no merge of the app-state entry, so the
/// answer is `Err` and repair resolves the entry by last-writer-wins.
#[actix::test]
async fn a_module_without_the_root_merge_export_answers_that_it_holds_no_merge() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
    let request = MergeRootStateRequest {
        existing: b"stored".to_vec(),
        incoming: b"peer".to_vec(),
        existing_created_at: 1,
        existing_ts: 1,
        incoming_ts: 2,
    };

    let answer = fx
        .harness
        .context_client
        .merge_root_state(&fx.context_id, &fx.executor, request)
        .await;

    assert!(
        matches!(answer, Ok(MergeRootStateResponse::Err(_))),
        "a missing export must answer that the module holds no merge, got {answer:?}"
    );
}

/// Only a missing export is marked; any other failure stays a failed run, which
/// the caller retries rather than resolving by last-writer-wins.
#[test]
fn only_a_missing_export_is_marked_and_its_message_is_unchanged() {
    let missing = || {
        FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
            name: "__calimero_merge_root_state".to_owned(),
        })
    };
    let other = FunctionCallError::MethodResolutionError(MethodResolutionError::EmptyMethodName);

    let marked = returns_error(missing());
    let unmarked = returns_error(other);

    assert!(marked.downcast_ref::<MethodNotExported>().is_some());
    assert_eq!(marked.to_string(), missing().to_string());
    assert!(unmarked.downcast_ref::<MethodNotExported>().is_none());
}
