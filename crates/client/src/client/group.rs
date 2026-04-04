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
use calimero_server_primitives::admin::GroupInfoApiResponse;
use calimero_server_primitives::admin::JoinContextApiResponse;
use calimero_server_primitives::admin::ListGroupContextsApiResponse;
use calimero_server_primitives::admin::ListGroupMembersApiResponse;
use calimero_server_primitives::admin::ListSubgroupsApiResponse;
use calimero_server_primitives::admin::NestGroupApiRequest;
use calimero_server_primitives::admin::NestGroupApiResponse;
use calimero_server_primitives::admin::RegisterGroupSigningKeyApiRequest;
use calimero_server_primitives::admin::RegisterGroupSigningKeyApiResponse;
use calimero_server_primitives::admin::RemoveGroupMembersApiRequest;
use calimero_server_primitives::admin::RemoveGroupMembersApiResponse;
use calimero_server_primitives::admin::RetryGroupUpgradeApiRequest;
use calimero_server_primitives::admin::SetDefaultCapabilitiesApiRequest;
use calimero_server_primitives::admin::SetDefaultCapabilitiesApiResponse;
use calimero_server_primitives::admin::SetDefaultVisibilityApiRequest;
use calimero_server_primitives::admin::SetDefaultVisibilityApiResponse;
use calimero_server_primitives::admin::SetMemberCapabilitiesApiRequest;
use calimero_server_primitives::admin::SetMemberCapabilitiesApiResponse;
use calimero_server_primitives::admin::SyncGroupApiRequest;
use calimero_server_primitives::admin::SyncGroupApiResponse;
use calimero_server_primitives::admin::UnnestGroupApiRequest;
use calimero_server_primitives::admin::UnnestGroupApiResponse;
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

    pub async fn nest_group(
        &self,
        parent_group_id: &str,
        request: NestGroupApiRequest,
    ) -> Result<NestGroupApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{parent_group_id}/nest"), request)
            .await?;
        Ok(response)
    }

    pub async fn unnest_group(
        &self,
        parent_group_id: &str,
        request: UnnestGroupApiRequest,
    ) -> Result<UnnestGroupApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{parent_group_id}/unnest"), request)
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

    pub async fn set_default_visibility(
        &self,
        group_id: &str,
        request: SetDefaultVisibilityApiRequest,
    ) -> Result<SetDefaultVisibilityApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/settings/default-visibility"),
                request,
            )
            .await?;
        Ok(response)
    }
}
