use actix::Handler;
use calimero_context_primitives::messages::ContextMessage;
use calimero_utils_actix::adapters::ActorExt;

use crate::ContextManager;

pub mod add_group_members;
pub mod admit_tee_node;
pub mod apply_signed_group_op;
pub mod broadcast_group_aliases;
pub mod broadcast_group_local_state;
pub mod create_context;
pub mod create_group;
pub mod create_group_invitation;
pub mod delete_context;
pub mod delete_group;
pub mod detach_context_from_group;
pub mod execute;
pub mod get_context_allowlist;
pub mod get_context_visibility;
pub mod get_group_for_context;
pub mod get_group_info;
pub mod get_group_upgrade_status;
pub mod get_member_capabilities;
pub mod get_namespace_identity;
pub mod join_context;
pub mod join_group;
pub mod join_group_context;
pub mod list_all_groups;
pub mod list_group_contexts;
pub mod list_group_members;
pub mod list_namespaces;
pub mod list_namespaces_for_application;
pub mod manage_context_allowlist;
pub mod remove_group_members;
pub mod retry_group_upgrade;
pub mod set_context_visibility;
pub mod set_default_capabilities;
pub mod set_default_visibility;
pub mod set_group_alias;
pub mod set_member_alias;
pub mod set_member_capabilities;
pub mod set_tee_admission_policy;
pub mod store_context_alias;
pub mod store_context_allowlist;
pub mod store_context_visibility;
pub mod store_default_capabilities;
pub mod store_default_visibility;
pub mod store_group_alias;
pub mod store_group_context;
pub mod store_group_meta;
pub mod store_member_alias;
pub mod store_member_capability;
pub mod sync;
pub mod sync_group;
pub mod update_application;
pub mod update_group_settings;
pub mod update_member_role;
pub mod upgrade_group;

impl Handler<ContextMessage> for ContextManager {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            ContextMessage::Execute { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpdateApplication { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::CreateContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::DeleteContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::Sync { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::CreateGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::DeleteGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::AddGroupMembers { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ApplySignedGroupOp { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::RemoveGroupMembers { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetGroupInfo { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListGroupMembers { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListGroupContexts { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpgradeGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetGroupUpgradeStatus { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::RetryGroupUpgrade { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::CreateGroupInvitation { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListAllGroups { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpdateGroupSettings { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpdateMemberRole { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::DetachContextFromGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetGroupForContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SyncGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinGroupContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SetMemberCapabilities { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetMemberCapabilities { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SetContextVisibility { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetContextVisibility { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ManageContextAllowlist { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetContextAllowlist { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SetDefaultCapabilities { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SetTeeAdmissionPolicy { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::AdmitTeeNode { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SetDefaultVisibility { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreContextAlias { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::BroadcastGroupAliases { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::BroadcastGroupLocalState { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreMemberCapability { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreDefaultCapabilities { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreContextVisibility { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreDefaultVisibility { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreContextAllowlist { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SetMemberAlias { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreMemberAlias { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SetGroupAlias { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreGroupAlias { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreGroupContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreGroupMeta { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListNamespaces { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetNamespaceIdentity { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListNamespacesForApplication { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
        }
    }
}
