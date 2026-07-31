use calimero_context_config::MemberCapabilities;
use calimero_server_primitives::admin::{
    AddGroupMembersApiResponse, CreateAccountApiResponse, CreateGroupApiResponse,
    CreateGroupInvitationApiResponse, CreateNamespaceApiResponse, DeleteGroupApiResponse,
    DeleteNamespaceApiResponse, DetachContextFromGroupApiResponse,
    GetGroupUpgradeStatusApiResponse, GetMemberCapabilitiesApiResponse, GetMetadataApiResponse,
    GroupInfoApiResponse, JoinContextApiResponse, JoinGroupApiResponse, LeaveContextApiResponse,
    LeaveGroupApiResponse, LeaveNamespaceApiResponse, ListGroupContextsApiResponse,
    ListGroupMembersApiResponse, ListNamespaceGroupsApiResponse, ListNamespacesApiResponse,
    ListSubgroupsApiResponse, NamespaceApiResponse, NamespaceIdentityApiResponse,
    PairDeviceCompleteApiResponse, PairDeviceInitApiResponse, RegisterGroupSigningKeyApiResponse,
    RemoveGroupMembersApiResponse, ReparentGroupApiResponse, RevokeDeviceApiResponse,
    SetDefaultCapabilitiesApiResponse, SetMemberCapabilitiesApiResponse, SetMetadataApiResponse,
    SetSubgroupVisibilityApiResponse, SyncGroupApiResponse, UpdateGroupSettingsApiResponse,
    UpdateMemberRoleApiResponse, UpgradeGroupApiResponse,
};
use color_eyre::owo_colors::OwoColorize;
use comfy_table::{Cell, Color, Table};

use super::Report;

impl Report for CreateGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Group Created").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Group ID", &self.data.group_id]);
        println!("{table}");
    }
}

impl Report for GroupInfoApiResponse {
    fn report(&self) {
        let d = &self.data;
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Field").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Group ID", &d.group_id]);
        let _ = table.add_row(vec!["App Key", &d.app_key]);
        let _ = table.add_row(vec!["Application ID", &d.target_application_id.to_string()]);
        let _ = table.add_row(vec!["Upgrade Policy", &format!("{:?}", d.upgrade_policy)]);
        let _ = table.add_row(vec!["Members", &d.member_count.to_string()]);
        let _ = table.add_row(vec!["Contexts", &d.context_count.to_string()]);
        if let Some(meta) = d.metadata.as_ref() {
            if let Some(ref name) = meta.name {
                let _ = table.add_row(vec!["Name", name]);
            }
            if !meta.data.is_empty() {
                let _ = table.add_row(vec!["Data".to_owned(), format!("{} keys", meta.data.len())]);
            }
        }
        if let Some(ref upgrade) = d.active_upgrade {
            let _ = table.add_row(vec!["Active Upgrade Status", &upgrade.status]);
        }
        println!("{table}");
    }
}

impl Report for CreateAccountApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Device Enrolled").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Account ID", &self.data.account_id]);
        let _ = table.add_row(vec!["Device ID", &self.data.device_id]);
        let _ = table.add_row(vec!["Account root key", &self.data.account_root_key]);
        // Shown because pairing a second device needs it: the other device
        // computes its own id as H(account ‖ nonce), so without the nonce the
        // account cannot be joined at all.
        let _ = table.add_row(vec![
            "Account nonce (for pairing)",
            &self.data.account_nonce,
        ]);
        println!("{table}");
    }
}

impl Report for PairDeviceInitApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Device Minted").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Account ID", &self.data.account_id]);
        // All three are what `account pair-complete` needs on the other device.
        // The device is inert until that runs: it holds no key, and nothing has
        // certified it, so printing them is the whole point of this half.
        let _ = table.add_row(vec!["Device ID", &self.data.device_id]);
        let _ = table.add_row(vec!["Device KEM key", &self.data.kem_public_key]);
        let _ = table.add_row(vec!["Device signing key", &self.data.sign_public_key]);
        let _ = table.add_row(vec!["Pairing statement", &self.data.statement]);
        // Read aloud, not transcribed: the point of the code is that a person
        // compares it against the other machine's. Anything the payload carries
        // is something an attacker rewriting the payload also controls.
        let _ = table.add_row(vec![
            "Confirmation code (read aloud)",
            &self.data.confirmation_code,
        ]);
        println!("{table}");
    }
}

impl Report for PairDeviceCompleteApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Device Paired").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Account ID", &self.data.account_id]);
        let _ = table.add_row(vec!["Device ID", &self.data.device_id]);
        // Surfaced rather than folded into a flat success: the link confers
        // authority on its own, but until the key lands the device cannot read.
        let _ = table.add_row(vec![
            "Scope key delivered",
            if self.data.key_delivered {
                "yes"
            } else {
                "no - the device's sync pull will retry"
            },
        ]);
        let _ = table.add_row(vec![
            "Confirmation code (must match pair-init)",
            &self.data.confirmation_code,
        ]);
        println!("{table}");
    }
}

impl Report for RevokeDeviceApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Device Revoked").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Account ID", &self.data.account_id]);
        let _ = table.add_row(vec!["Device ID", &self.data.device_id]);
        // Without the rotation the device stops writing but keeps the key it
        // already holds — a silent reader. Say so rather than report a bare
        // success.
        let _ = table.add_row(vec![
            "Scope key rotated",
            if self.data.key_rotated {
                "yes - the device can no longer read either"
            } else {
                "no - it can still READ until an admin rotates"
            },
        ]);
        println!("{table}");
    }
}

impl Report for CreateNamespaceApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Namespace Created").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Namespace ID", &self.data.namespace_id]);
        println!("{table}");
    }
}

impl Report for DeleteNamespaceApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Namespace Deleted").fg(Color::Green)]);
        let _ = table.add_row(vec![format!(
            "Successfully deleted namespace (deleted: {})",
            self.data.is_deleted
        )]);
        println!("{table}");
    }
}

impl Report for NamespaceApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Field").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Namespace ID", &self.namespace_id]);
        let _ = table.add_row(vec!["App Key", &self.app_key]);
        let _ = table.add_row(vec!["Application ID", &self.target_application_id]);
        let _ = table.add_row(vec!["Upgrade Policy", &self.upgrade_policy]);
        let _ = table.add_row(vec!["Created At", &self.created_at.to_string()]);
        let _ = table.add_row(vec!["Members", &self.member_count.to_string()]);
        let _ = table.add_row(vec!["Contexts", &self.context_count.to_string()]);
        let _ = table.add_row(vec!["Subgroups", &self.subgroup_count.to_string()]);
        if let Some(ref name) = self.name {
            let _ = table.add_row(vec!["Name", name]);
        }
        println!("{table}");
    }
}

impl Report for ListNamespacesApiResponse {
    fn report(&self) {
        if self.data.is_empty() {
            println!("No namespaces found");
            return;
        }

        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Namespace ID").fg(Color::Blue),
            Cell::new("Application ID").fg(Color::Blue),
            Cell::new("Upgrade Policy").fg(Color::Blue),
            Cell::new("Members").fg(Color::Blue),
            Cell::new("Contexts").fg(Color::Blue),
            Cell::new("Subgroups").fg(Color::Blue),
            Cell::new("Name").fg(Color::Blue),
        ]);
        for ns in &self.data {
            let _ = table.add_row(vec![
                ns.namespace_id.clone(),
                ns.target_application_id.clone(),
                ns.upgrade_policy.clone(),
                ns.member_count.to_string(),
                ns.context_count.to_string(),
                ns.subgroup_count.to_string(),
                ns.name.clone().unwrap_or_else(|| "-".to_owned()),
            ]);
        }
        println!("{table}");
    }
}

impl Report for NamespaceIdentityApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Namespace Identity").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Namespace ID", &self.namespace_id]);
        let _ = table.add_row(vec!["Public Key", &self.public_key]);
        println!("{table}");
    }
}

impl Report for ListNamespaceGroupsApiResponse {
    fn report(&self) {
        if self.data.is_empty() {
            println!("No groups found in namespace");
            return;
        }

        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Group ID").fg(Color::Blue),
            Cell::new("Name").fg(Color::Blue),
        ]);
        for group in &self.data {
            let _ = table.add_row(vec![
                group.group_id.clone(),
                group.name.clone().unwrap_or_else(|| "-".to_owned()),
            ]);
        }
        println!("{table}");
    }
}

impl Report for ListSubgroupsApiResponse {
    fn report(&self) {
        if self.subgroups.is_empty() {
            println!("No subgroups found");
            return;
        }

        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Group ID").fg(Color::Blue),
            Cell::new("Name").fg(Color::Blue),
        ]);
        for group in &self.subgroups {
            let _ = table.add_row(vec![
                group.group_id.clone(),
                group.name.clone().unwrap_or_else(|| "-".to_owned()),
            ]);
        }
        println!("{table}");
    }
}

impl Report for ReparentGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Group Reparented").fg(Color::Green)]);
        let _ = table.add_row(vec![if self.reparented {
            "Successfully reparented group"
        } else {
            "Reparent did not change parent"
        }]);
        println!("{table}");
    }
}

impl Report for DeleteGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Group Deleted").fg(Color::Green)]);
        let _ = table.add_row(vec![format!(
            "Successfully deleted group (deleted: {})",
            self.data.is_deleted
        )]);
        println!("{table}");
    }
}

impl Report for UpdateGroupSettingsApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Group Settings Updated").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully updated group settings"]);
        println!("{table}");
    }
}

impl Report for ListGroupMembersApiResponse {
    fn report(&self) {
        if self.members.is_empty() {
            println!("No members found in group");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Identity").fg(Color::Blue),
                Cell::new("Role").fg(Color::Blue),
                Cell::new("Name").fg(Color::Blue),
            ]);
            for member in &self.members {
                let _ = table.add_row(vec![
                    member.identity.to_string(),
                    format!("{:?}", member.role),
                    member.name.clone().unwrap_or_else(|| "-".to_owned()),
                ]);
            }
            println!("{table}");
        }
    }
}

impl Report for AddGroupMembersApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Members Added").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully added members to group"]);
        println!("{table}");
    }
}

impl Report for RemoveGroupMembersApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Members Removed").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully removed members from group"]);
        println!("{table}");
    }
}

impl Report for UpdateMemberRoleApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Member Role Updated").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully updated member role"]);
        println!("{table}");
    }
}

impl Report for ListGroupContextsApiResponse {
    fn report(&self) {
        if self.data.is_empty() {
            println!("No contexts found in group");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Context ID").fg(Color::Blue),
                Cell::new("Name").fg(Color::Blue),
            ]);
            for entry in &self.data {
                let name = entry.name.as_deref().unwrap_or("-");
                let _ = table.add_row(vec![entry.context_id.clone(), name.to_owned()]);
            }
            println!("{table}");
        }
    }
}

impl Report for DetachContextFromGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Detached").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully detached context from group"]);
        println!("{table}");
    }
}

impl Report for CreateGroupInvitationApiResponse {
    fn report(&self) {
        println!("{}", "Group Invitation Created Successfully".green());
        println!();
        let pretty = serde_json::to_string_pretty(&self.data.invitation)
            .unwrap_or_else(|_| format!("{:?}", self.data.invitation));
        println!("{pretty}");
        println!();
        let compact = serde_json::to_string(&self.data.invitation).unwrap_or_default();
        println!("To join, run from another node:");
        println!("  meroctl --node <NODE_ID> group join '{compact}'");
    }
}

impl Report for JoinGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Group Joined").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Group ID", &self.data.group_id]);
        let _ = table.add_row(vec![
            "Member Identity",
            &self.data.member_identity.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for RegisterGroupSigningKeyApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Signing Key Registered").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Public Key", &self.data.public_key.to_string()]);
        println!("{table}");
    }
}

impl Report for UpgradeGroupApiResponse {
    fn report(&self) {
        let d = &self.data;
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Group Upgrade").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Group ID", &d.group_id]);
        let _ = table.add_row(vec!["Status", &d.status]);
        if let Some(total) = d.total {
            let _ = table.add_row(vec!["Total", &total.to_string()]);
        }
        if let Some(completed) = d.completed {
            let _ = table.add_row(vec!["Completed", &completed.to_string()]);
        }
        if let Some(failed) = d.failed {
            let _ = table.add_row(vec!["Failed", &failed.to_string()]);
        }
        println!("{table}");
    }
}

impl Report for GetGroupUpgradeStatusApiResponse {
    fn report(&self) {
        match &self.data {
            None => println!("No active upgrade in progress"),
            Some(upgrade) => {
                let mut table = Table::new();
                let _ = table.set_header(vec![
                    Cell::new("Upgrade Status").fg(Color::Blue),
                    Cell::new("Value").fg(Color::Blue),
                ]);
                let _ = table.add_row(vec!["From Version", &upgrade.from_version]);
                let _ = table.add_row(vec!["To Version", &upgrade.to_version]);
                let _ = table.add_row(vec!["Status", &upgrade.status]);
                let _ = table.add_row(vec!["Initiated By", &upgrade.initiated_by.to_string()]);
                let _ = table.add_row(vec!["Initiated At", &upgrade.initiated_at.to_string()]);
                if let Some(total) = upgrade.total {
                    let _ = table.add_row(vec!["Total", &total.to_string()]);
                }
                if let Some(completed) = upgrade.completed {
                    let _ = table.add_row(vec!["Completed", &completed.to_string()]);
                }
                if let Some(failed) = upgrade.failed {
                    let _ = table.add_row(vec!["Failed", &failed.to_string()]);
                }
                if let Some(completed_at) = upgrade.completed_at {
                    let _ = table.add_row(vec!["Completed At", &completed_at.to_string()]);
                }
                println!("{table}");
            }
        }
    }
}

impl Report for JoinContextApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Joined Context via Group").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Context ID", &self.data.context_id.to_string()]);
        let _ = table.add_row(vec![
            "Member Public Key",
            &self.data.member_public_key.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for LeaveContextApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Left Context (local-only)").fg(Color::Yellow),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Context ID", &self.data.context_id.to_string()]);
        let _ = table.add_row(vec![
            "Member Public Key",
            &self.data.member_public_key.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for LeaveGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Left Group (MemberLeft published)").fg(Color::Yellow),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Group ID", &self.data.group_id]);
        let _ = table.add_row(vec![
            "Member Public Key",
            &self.data.member_public_key.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for LeaveNamespaceApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Left Namespace (cascaded through descendants)").fg(Color::Yellow),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Namespace ID", &self.data.namespace_id]);
        let _ = table.add_row(vec![
            "Member Public Key",
            &self.data.member_public_key.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for SyncGroupApiResponse {
    fn report(&self) {
        let d = &self.data;
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Group Synced").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Group ID", &d.group_id]);
        let _ = table.add_row(vec!["App Key", &d.app_key]);
        let _ = table.add_row(vec!["Application ID", &d.target_application_id.to_string()]);
        let _ = table.add_row(vec!["Members", &d.member_count.to_string()]);
        let _ = table.add_row(vec!["Contexts", &d.context_count.to_string()]);
        println!("{table}");
    }
}

// ---- Group Permissions ----

impl Report for SetMemberCapabilitiesApiResponse {
    fn report(&self) {
        println!("{}", "Member capabilities updated successfully".green());
    }
}

impl Report for GetMemberCapabilitiesApiResponse {
    fn report(&self) {
        let caps = self.data.capabilities;
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Capability").fg(Color::Blue),
            Cell::new("Enabled").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec![
            "CAN_CREATE_CONTEXT".to_owned(),
            if caps & MemberCapabilities::CAN_CREATE_CONTEXT.bits() != 0 {
                "yes"
            } else {
                "no"
            }
            .to_owned(),
        ]);
        let _ = table.add_row(vec![
            "CAN_INVITE_MEMBERS".to_owned(),
            if caps & MemberCapabilities::CAN_INVITE_MEMBERS.bits() != 0 {
                "yes"
            } else {
                "no"
            }
            .to_owned(),
        ]);
        let _ = table.add_row(vec![
            "CAN_JOIN_OPEN_SUBGROUPS".to_owned(),
            if caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits() != 0 {
                "yes"
            } else {
                "no"
            }
            .to_owned(),
        ]);
        let _ = table.add_row(vec![
            "Raw value".to_owned(),
            format!("{caps} (0b{caps:03b})"),
        ]);
        println!("{table}");
    }
}

impl Report for SetDefaultCapabilitiesApiResponse {
    fn report(&self) {
        println!(
            "{}",
            "Default member capabilities updated successfully".green()
        );
    }
}

impl Report for SetSubgroupVisibilityApiResponse {
    fn report(&self) {
        println!("{}", "Subgroup visibility updated successfully".green());
    }
}

impl Report for GetMetadataApiResponse {
    fn report(&self) {
        let Some(record) = self.data.as_ref() else {
            println!("{}", "(no metadata set)".dimmed());
            return;
        };
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Field").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec![
            "Name".to_owned(),
            record.name.clone().unwrap_or_else(|| "-".to_owned()),
        ]);
        let _ = table.add_row(vec!["Updated At".to_owned(), record.updated_at.to_string()]);
        let _ = table.add_row(vec!["Updated By".to_owned(), record.updated_by.to_string()]);
        if record.data.is_empty() {
            let _ = table.add_row(vec!["Data".to_owned(), "(empty)".to_owned()]);
        } else {
            for (k, v) in &record.data {
                let _ = table.add_row(vec![format!("data.{k}"), v.clone()]);
            }
        }
        println!("{table}");
    }
}

impl Report for SetMetadataApiResponse {
    fn report(&self) {
        println!("{}", "Metadata updated successfully".green());
    }
}
