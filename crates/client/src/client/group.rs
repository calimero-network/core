//! Group API methods for the Calimero client.

use eyre::Result;

use calimero_server_primitives::admin::AddGroupMembersApiRequest;
use calimero_server_primitives::admin::AddGroupMembersApiResponse;
use calimero_server_primitives::admin::ClaimGroupInvitationApiRequest;
use calimero_server_primitives::admin::ClaimGroupInvitationApiResponse;
use calimero_server_primitives::admin::DeleteGroupApiRequest;
use calimero_server_primitives::admin::DeleteGroupApiResponse;
use calimero_server_primitives::admin::DetachContextFromGroupApiRequest;
use calimero_server_primitives::admin::DetachContextFromGroupApiResponse;
use calimero_server_primitives::admin::GetGroupUpgradeStatusApiResponse;
use calimero_server_primitives::admin::GetMemberCapabilitiesApiResponse;
use calimero_server_primitives::admin::GetMetadataApiResponse;
use calimero_server_primitives::admin::GetTeeAdmissionPolicyApiResponse;
use calimero_server_primitives::admin::GroupInfoApiResponse;
use calimero_server_primitives::admin::JoinContextApiResponse;
use calimero_server_primitives::admin::JoinSubgroupInheritanceApiResponse;
use calimero_server_primitives::admin::LeaveContextApiResponse;
use calimero_server_primitives::admin::LeaveGroupApiResponse;
use calimero_server_primitives::admin::LeaveNamespaceApiResponse;
use calimero_server_primitives::admin::ListGroupContextsApiResponse;
use calimero_server_primitives::admin::ListGroupMembersApiResponse;
use calimero_server_primitives::admin::ListSubgroupsApiResponse;
use calimero_server_primitives::admin::RegisterGroupSigningKeyApiRequest;
use calimero_server_primitives::admin::RegisterGroupSigningKeyApiResponse;
use calimero_server_primitives::admin::RemoveGroupMembersApiRequest;
use calimero_server_primitives::admin::RemoveGroupMembersApiResponse;
use calimero_server_primitives::admin::ReparentGroupApiRequest;
use calimero_server_primitives::admin::ReparentGroupApiResponse;
use calimero_server_primitives::admin::RetryGroupUpgradeApiRequest;
use calimero_server_primitives::admin::SetDefaultCapabilitiesApiRequest;
use calimero_server_primitives::admin::SetDefaultCapabilitiesApiResponse;
use calimero_server_primitives::admin::SetMemberAutoFollowApiRequest;
use calimero_server_primitives::admin::SetMemberAutoFollowApiResponse;
use calimero_server_primitives::admin::SetMemberCapabilitiesApiRequest;
use calimero_server_primitives::admin::SetMemberCapabilitiesApiResponse;
use calimero_server_primitives::admin::SetMetadataApiRequest;
use calimero_server_primitives::admin::SetMetadataApiResponse;
use calimero_server_primitives::admin::SetSubgroupVisibilityApiRequest;
use calimero_server_primitives::admin::SetSubgroupVisibilityApiResponse;
use calimero_server_primitives::admin::SetTeeAdmissionPolicyApiRequest;
use calimero_server_primitives::admin::SetTeeAdmissionPolicyApiResponse;
use calimero_server_primitives::admin::SyncGroupApiRequest;
use calimero_server_primitives::admin::SyncGroupApiResponse;
use calimero_server_primitives::admin::UpdateGroupSettingsApiRequest;
use calimero_server_primitives::admin::UpdateGroupSettingsApiResponse;
use calimero_server_primitives::admin::UpdateMemberRoleApiRequest;
use calimero_server_primitives::admin::UpdateMemberRoleApiResponse;
use calimero_server_primitives::admin::UpgradeGroupApiRequest;
use calimero_server_primitives::admin::UpgradeGroupApiResponse;

use crate::traits::ClientAuthenticator;
use crate::traits::ClientStorage;

impl<A, S> super::Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn get_group_info(&self, group_id: &str) -> Result<GroupInfoApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}"))
            .await?;
        Ok(response)
    }

    pub async fn delete_group(
        &self,
        group_id: &str,
        request: DeleteGroupApiRequest,
    ) -> Result<DeleteGroupApiResponse> {
        let response = self
            .connection
            .delete_with_body(&format!("admin-api/groups/{group_id}"), request)
            .await?;
        Ok(response)
    }

    pub async fn update_group_settings(
        &self,
        group_id: &str,
        request: UpdateGroupSettingsApiRequest,
    ) -> Result<UpdateGroupSettingsApiResponse> {
        let response = self
            .connection
            .patch(&format!("admin-api/groups/{group_id}"), request)
            .await?;
        Ok(response)
    }

    pub async fn list_group_members(&self, group_id: &str) -> Result<ListGroupMembersApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/members"))
            .await?;
        Ok(response)
    }

    pub async fn add_group_members(
        &self,
        group_id: &str,
        request: AddGroupMembersApiRequest,
    ) -> Result<AddGroupMembersApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/members"), request)
            .await?;
        Ok(response)
    }

    pub async fn remove_group_members(
        &self,
        group_id: &str,
        request: RemoveGroupMembersApiRequest,
    ) -> Result<RemoveGroupMembersApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/members/remove"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn update_member_role(
        &self,
        group_id: &str,
        identity_hex: &str,
        request: UpdateMemberRoleApiRequest,
    ) -> Result<UpdateMemberRoleApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/members/{identity_hex}/role"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn list_group_contexts(
        &self,
        group_id: &str,
    ) -> Result<ListGroupContextsApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/contexts"))
            .await?;
        Ok(response)
    }

    pub async fn detach_context_from_group(
        &self,
        group_id: &str,
        context_id: &str,
        request: DetachContextFromGroupApiRequest,
    ) -> Result<DetachContextFromGroupApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/contexts/{context_id}/remove"),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Atomic edge swap: move `group_id` to a new parent. Replaces the
    /// previous nest/unnest pair — orphan state is no longer reachable.
    pub async fn reparent_group(
        &self,
        group_id: &str,
        request: ReparentGroupApiRequest,
    ) -> Result<ReparentGroupApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/reparent"), request)
            .await?;
        Ok(response)
    }

    pub async fn list_subgroups(&self, group_id: &str) -> Result<ListSubgroupsApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/subgroups"))
            .await?;
        Ok(response)
    }

    pub async fn claim_group_invitation(
        &self,
        request: ClaimGroupInvitationApiRequest,
    ) -> Result<ClaimGroupInvitationApiResponse> {
        let response = self
            .connection
            .post("admin-api/groups/claim-invitation", request)
            .await?;
        Ok(response)
    }

    pub async fn register_group_signing_key(
        &self,
        group_id: &str,
        request: RegisterGroupSigningKeyApiRequest,
    ) -> Result<RegisterGroupSigningKeyApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/signing-key"), request)
            .await?;
        Ok(response)
    }

    pub async fn upgrade_group(
        &self,
        group_id: &str,
        request: UpgradeGroupApiRequest,
    ) -> Result<UpgradeGroupApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/upgrade"), request)
            .await?;
        Ok(response)
    }

    pub async fn get_group_upgrade_status(
        &self,
        group_id: &str,
    ) -> Result<GetGroupUpgradeStatusApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/upgrade/status"))
            .await?;
        Ok(response)
    }

    pub async fn retry_group_upgrade(
        &self,
        group_id: &str,
        request: RetryGroupUpgradeApiRequest,
    ) -> Result<UpgradeGroupApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/upgrade/retry"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn sync_group(
        &self,
        group_id: &str,
        request: SyncGroupApiRequest,
    ) -> Result<SyncGroupApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/sync"), request)
            .await?;
        Ok(response)
    }

    pub async fn join_context(&self, context_id: &str) -> Result<JoinContextApiResponse> {
        let response = self
            .connection
            .post_no_body(&format!("admin-api/contexts/{context_id}/join"))
            .await?;
        Ok(response)
    }

    /// Materialise an inherited Open-subgroup membership directly,
    /// without an admin-signed invitation and without joining a child
    /// context first. See core PR #2357 for the endpoint contract.
    pub async fn join_subgroup_inheritance(
        &self,
        group_id: &str,
    ) -> Result<JoinSubgroupInheritanceApiResponse> {
        let response = self
            .connection
            .post_no_body(&format!("admin-api/groups/{group_id}/join-via-inheritance"))
            .await?;
        Ok(response)
    }

    /// Local-only opt-out from a single context. Stops sync and disarms
    /// auto-follow on this node only — peers do not observe the leave.
    /// Reversal: call [`Self::join_context`] which clears the marker.
    /// See `architecture/membership-and-leave.html` § 4 for semantics.
    pub async fn leave_context(&self, context_id: &str) -> Result<LeaveContextApiResponse> {
        let response = self
            .connection
            .post_no_body(&format!("admin-api/contexts/{context_id}/leave"))
            .await?;
        Ok(response)
    }

    /// Distributed self-leave from a single group. Publishes a
    /// `MemberLeft` op that all peers apply, deleting the leaver's
    /// direct membership row. Subject to apply-side validation:
    /// signer must be a direct member, must not be Owner, and must
    /// not be the only admin (`LastAdmin` rejection). See
    /// `architecture/membership-and-leave.html` § 5.
    pub async fn leave_group(&self, group_id: &str) -> Result<LeaveGroupApiResponse> {
        let response = self
            .connection
            .post_no_body(&format!("admin-api/groups/{group_id}/leave"))
            .await?;
        Ok(response)
    }

    /// Self-leave from a namespace (root group). Cascades through
    /// every descendant where the leaver has a direct row; multi-scope
    /// owner + last-admin checks run upfront. Rejects with
    /// `MustTransferOwnership` if the leaver owns any group in the
    /// subtree. See `architecture/membership-and-leave.html` § 6.
    pub async fn leave_namespace(&self, namespace_id: &str) -> Result<LeaveNamespaceApiResponse> {
        let response = self
            .connection
            .post_no_body(&format!("admin-api/namespaces/{namespace_id}/leave"))
            .await?;
        Ok(response)
    }

    // ---- Group Permissions ----

    pub async fn set_member_capabilities(
        &self,
        group_id: &str,
        identity_hex: &str,
        request: SetMemberCapabilitiesApiRequest,
    ) -> Result<SetMemberCapabilitiesApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/members/{identity_hex}/capabilities"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn get_member_capabilities(
        &self,
        group_id: &str,
        identity_hex: &str,
    ) -> Result<GetMemberCapabilitiesApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/groups/{group_id}/members/{identity_hex}/capabilities"
            ))
            .await?;
        Ok(response)
    }

    /// Toggle a member's per-group auto-follow flags
    /// (`auto_follow.contexts` / `auto_follow.subgroups`).
    ///
    /// Authorized by group admin (for any `identity_hex`) or by the target
    /// itself (self-setting). The apply path enforces the admin-or-self rule —
    /// see `GroupOp::MemberSetAutoFollow`.
    pub async fn set_member_auto_follow(
        &self,
        group_id: &str,
        identity_hex: &str,
        request: SetMemberAutoFollowApiRequest,
    ) -> Result<SetMemberAutoFollowApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/members/{identity_hex}/auto-follow"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn set_default_capabilities(
        &self,
        group_id: &str,
        request: SetDefaultCapabilitiesApiRequest,
    ) -> Result<SetDefaultCapabilitiesApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/settings/default-capabilities"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn set_subgroup_visibility(
        &self,
        group_id: &str,
        request: SetSubgroupVisibilityApiRequest,
    ) -> Result<SetSubgroupVisibilityApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/settings/subgroup-visibility"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn get_tee_admission_policy(
        &self,
        group_id: &str,
    ) -> Result<GetTeeAdmissionPolicyApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/groups/{group_id}/settings/tee-admission-policy"
            ))
            .await?;
        Ok(response)
    }

    pub async fn set_tee_admission_policy(
        &self,
        group_id: &str,
        request: SetTeeAdmissionPolicyApiRequest,
    ) -> Result<SetTeeAdmissionPolicyApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/settings/tee-admission-policy"),
                request,
            )
            .await?;
        Ok(response)
    }

    // ---- Metadata records ----

    pub async fn get_group_metadata(&self, group_id: &str) -> Result<GetMetadataApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/metadata"))
            .await?;
        Ok(response)
    }

    pub async fn set_group_metadata(
        &self,
        group_id: &str,
        request: SetMetadataApiRequest,
    ) -> Result<SetMetadataApiResponse> {
        let response = self
            .connection
            .put_json(&format!("admin-api/groups/{group_id}/metadata"), request)
            .await?;
        Ok(response)
    }

    pub async fn get_member_metadata(
        &self,
        group_id: &str,
        identity_hex: &str,
    ) -> Result<GetMetadataApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/groups/{group_id}/members/{identity_hex}/metadata"
            ))
            .await?;
        Ok(response)
    }

    pub async fn set_member_metadata(
        &self,
        group_id: &str,
        identity_hex: &str,
        request: SetMetadataApiRequest,
    ) -> Result<SetMetadataApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/members/{identity_hex}/metadata"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn get_context_metadata(
        &self,
        group_id: &str,
        context_id: &str,
    ) -> Result<GetMetadataApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/groups/{group_id}/contexts/{context_id}/metadata"
            ))
            .await?;
        Ok(response)
    }

    pub async fn set_context_metadata(
        &self,
        group_id: &str,
        context_id: &str,
        request: SetMetadataApiRequest,
    ) -> Result<SetMetadataApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/contexts/{context_id}/metadata"),
                request,
            )
            .await?;
        Ok(response)
    }
}
