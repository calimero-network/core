pub mod abort_migration;
pub mod add_group_members;
pub mod create_group;
pub mod create_group_invitation;
pub mod delete_group;
pub mod detach_context_from_group;
pub mod get_cascade_status;
pub mod get_group_info;
pub mod get_group_upgrade_status;
pub mod get_member_capabilities;
pub mod get_migration_status;
pub mod get_tee_admission_policy;
pub mod issue_namespace_ownership_proof;
pub mod issue_ownership_proof;
pub mod join_group;
pub mod join_subgroup_inheritance;
pub mod leave_group;
pub mod list_group_contexts;
pub mod list_group_members;
pub mod list_member_devices;
pub mod list_subgroups;
pub mod remove_group_members;
pub mod reparent_group;
pub mod retry_group_upgrade;
pub mod set_context_metadata;
pub mod set_default_capabilities;
pub mod set_group_metadata;
pub mod set_member_auto_follow;
pub mod set_member_capabilities;
pub mod set_member_metadata;
pub mod set_subgroup_visibility;
pub mod set_tee_admission_policy;
pub mod sync_group;
pub mod update_member_role;
pub mod upgrade_group;

use calimero_context_client::group::{GroupUpgradeInfo, GroupUpgradeStatus};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GroupUpgradeStatusApiData;
use reqwest::StatusCode;

use crate::admin::service::ApiError;

/// Hard cap on the page size accepted by the group/context list endpoints,
/// regardless of the caller-supplied `limit`. Keeps a single request's work
/// (and response size) bounded.
pub(crate) const MAX_LIST_LIMIT: usize = 1000;

/// Default page size when the caller omits `limit`.
pub(crate) const DEFAULT_LIST_LIMIT: usize = 100;

fn upgrade_info_to_api_data(info: &GroupUpgradeInfo) -> GroupUpgradeStatusApiData {
    let (status, local_total, local_swapped, local_failed, completed_at) = match &info.status {
        GroupUpgradeStatus::InProgress {
            total,
            completed,
            failed,
        } => (
            "in_progress",
            Some(*total),
            Some(*completed),
            Some(*failed),
            None,
        ),
        // `completed_at` is this node's own swap. Fleet convergence is a
        // different question, answered by the migration-status rollup.
        GroupUpgradeStatus::Completed { completed_at, .. } => {
            ("completed", None, None, None, *completed_at)
        }
    };

    GroupUpgradeStatusApiData {
        from_version: info.from_version.clone(),
        to_version: info.to_version.clone(),
        initiated_at: info.initiated_at,
        initiated_by: info.initiated_by,
        status: status.to_owned(),
        local_contexts_total: local_total,
        local_contexts_swapped: local_swapped,
        local_contexts_failed: local_failed,
        completed_at,
    }
}

pub fn parse_group_id(s: &str) -> Result<ContextGroupId, ApiError> {
    let bytes = hex::decode(s).map_err(|e| {
        // Keep the client message generic but preserve the parse cause server-side.
        tracing::debug!(error = %e, "parse_group_id: hex decode failed");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid group id format: expected hex-encoded 32 bytes".into(),
        }
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        tracing::debug!(len = bytes.len(), "parse_group_id: wrong byte length");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid group id: must be exactly 32 bytes".into(),
        }
    })?;
    Ok(ContextGroupId::from(arr))
}

/// Parse a context id from a path segment.
///
/// One parse, where there used to be two: `ContextId`'s own `FromStr` was tried
/// first and a manual `hex::decode` caught what it missed, because the type
/// spelled itself base58 while callers sent hex. Both halves are the same parse
/// now, so the fallback could only ever repeat the first one's answer.
pub(crate) fn parse_context_id(s: &str) -> Result<ContextId, ApiError> {
    s.parse::<ContextId>().map_err(|e| {
        tracing::debug!(error = %e, "parse_context_id: not a 32-byte hex id");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid context id format: expected 64 hex characters (32 bytes)".into(),
        }
    })
}

/// Parse a member ACCOUNT from a path segment.
///
/// Accounts render as 64 hex characters (see `AccountId`'s `Display`) — and so,
/// now, does a *key*. The two used to be told apart by encoding alone, so pasting
/// one where the other belongs was a 400; it no longer is, and this function
/// cannot detect it. See the test below, which pins the loss rather than hiding
/// it. Where the distinction has to survive, it is carried by a tag —
/// `MemberIdentity`'s `key:` prefix — not inferred from the spelling.
fn parse_account(s: &str) -> Result<calimero_account::AccountId, ApiError> {
    s.parse::<calimero_account::AccountId>()
        .map_err(|_| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid account format: expected 64 hex characters (32 bytes)".into(),
        })
}

#[cfg(test)]
mod tests {
    use calimero_primitives::identity::PublicKey;

    use super::{parse_account, parse_context_id};

    /// Base58 was accepted here and no longer is.
    ///
    /// `"1"` thirty-two times is base58 for 32 zero bytes, and was a valid
    /// context id at this endpoint. It is not hex, so it is now a 400.
    #[test]
    fn parse_context_id_refuses_base58_context_ids() {
        assert!(parse_context_id("11111111111111111111111111111111").is_err());
    }

    #[test]
    fn parse_context_id_keeps_accepting_hex_context_ids() {
        let context_id =
            parse_context_id("0000000000000000000000000000000000000000000000000000000000000000");

        assert!(context_id.is_ok());
    }

    #[test]
    fn parse_account_accepts_the_hex_an_account_renders_as() {
        assert!(
            parse_account("0000000000000000000000000000000000000000000000000000000000000000")
                .is_ok()
        );
    }

    /// A signing key now DOES parse as an account, and that is a real loss.
    ///
    /// This inverts `parse_account_rejects_a_bs58_signing_key`, which held that
    /// "the distinct encodings are what make that a 400 instead of a no-op".
    /// Exactly so — and the encodings are no longer distinct, so the 400 is gone:
    /// a key pasted into an account path segment now parses, names a principal
    /// that exists nowhere, and matches no member. The request succeeds and does
    /// nothing.
    ///
    /// Asserted rather than left implicit because it is the price of one
    /// encoding, and a silent no-op is the kind of thing that gets rediscovered
    /// as a bug. What survives the collapse survives by being tagged: see
    /// `MemberIdentity`, where a key is written `key:<hex>` precisely because
    /// this inference stopped working.
    #[test]
    fn parse_account_now_also_accepts_a_key_because_both_are_hex() {
        let key = PublicKey::from([0; 32]).to_string();

        assert!(
            parse_account(&key).is_ok(),
            "both render as 64 hex, so this can no longer tell them apart"
        );
    }
}
