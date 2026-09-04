//! Every body the server deserializes is a closed set: a client sending a field
//! the node no longer knows gets a 400, never a silent drop. A struct is listed
//! here when it gains `deny_unknown_fields`; a new request type belongs here too.

use calimero_server_primitives::{admin, jsonrpc, sse, ws};

/// serde reports an unknown key before it checks for missing fields, so a lone
/// unknown key distinguishes a closed type from an open one for every shape.
/// An adjacently tagged enum phrases the refusal as an invalid tag key.
macro_rules! rejects_unknown_fields {
    ($($ty:ty),* $(,)?) => {
        $({
            let err = serde_json::from_str::<$ty>(r#"{"bogus":1}"#)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(
                err.contains("unknown field `bogus`")
                    || err.contains(r#"string "bogus", expected "method" or "params""#),
                "{} accepts unknown fields: {err:?}",
                stringify!($ty)
            );
        })*
    };
}

#[test]
fn every_request_body_is_a_closed_set() {
    rejects_unknown_fields!(
        admin::InstallApplicationRequest,
        admin::InstallDevApplicationRequest,
        admin::CreateContextRequest,
        admin::UpdateContextApplicationRequest,
        admin::PerformIntentApiRequest,
        admin::ResyncContextApiRequest,
        admin::CreateGroupApiRequest,
        admin::DeleteGroupApiRequest,
        admin::ReparentGroupApiRequest,
        admin::AddGroupMembersApiRequest,
        admin::GroupMemberApiInput,
        admin::RemoveGroupMembersApiRequest,
        admin::UpdateMemberRoleApiRequest,
        admin::SetMetadataApiRequest,
        admin::DetachContextFromGroupApiRequest,
        admin::UpgradeGroupApiRequest,
        admin::RetryGroupUpgradeApiRequest,
        admin::IssueOwnershipProofApiRequest,
        admin::IssueNamespaceOwnershipProofApiRequest,
        admin::SyncGroupApiRequest,
        admin::SetMemberCapabilitiesApiRequest,
        admin::SetMemberAutoFollowApiRequest,
        admin::SetDefaultCapabilitiesApiRequest,
        admin::SetTeeAdmissionPolicyApiRequest,
        admin::SetSubgroupVisibilityApiRequest,
        admin::CreateGroupInvitationApiRequest,
        admin::JoinGroupApiRequest,
        admin::AccountPairInitApiRequest,
        admin::AccountPairCompleteApiRequest,
        admin::RelinkDeviceApiRequest,
        admin::RevokeDeviceApiRequest,
        admin::CreateNamespaceApiRequest,
        admin::DeleteNamespaceApiRequest,
        admin::AdmitJoinApiRequest,
        admin::TeeAttestRequest,
        admin::FleetJoinRequest,
        jsonrpc::RequestPayload,
        jsonrpc::ExecutionRequest,
        jsonrpc::SyncStatusRequest,
        jsonrpc::SetEphemeralRequest,
        ws::RequestPayload,
        ws::SubscribeRequest,
        ws::UnsubscribeRequest,
        sse::RequestPayload,
        sse::ContextIds,
    );
}

/// The envelope flattens its payload, so a stray sibling of `method`/`params`
/// lands in the payload and must be refused there.
#[test]
fn a_stray_envelope_key_is_refused_by_the_payload() {
    let err = serde_json::from_str::<jsonrpc::RequestPayload>(
        r#"{"method":"sync_status",
            "params":{"contextId":"0000000000000000000000000000000000000000000000000000000000000000"},
            "bogus":1}"#,
    )
    .expect_err("a stray key beside method/params must not deserialize");
    assert!(
        err.to_string()
            .contains(r#"string "bogus", expected "method" or "params""#),
        "got: {err}"
    );
}
