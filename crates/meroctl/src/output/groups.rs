use calimero_context_config::MemberCapabilities;
use calimero_server_primitives::admin::{
    AddGroupMembersApiResponse, CreateGroupApiResponse, CreateGroupInvitationApiResponse,
    CreateNamespaceApiResponse, DeleteGroupApiResponse, DeleteNamespaceApiResponse,
    DetachContextFromGroupApiResponse, GetGroupUpgradeStatusApiResponse,
    GetMemberCapabilitiesApiResponse, GetMetadataApiResponse, GroupInfoApiResponse,
    JoinContextApiResponse, JoinGroupApiResponse, JoinNamespaceApiResponse,
    LeaveContextApiResponse, LeaveGroupApiResponse, LeaveNamespaceApiResponse,
    ListGroupContextsApiResponse, ListGroupMembersApiResponse, ListNamespaceGroupsApiResponse,
    ListNamespacesApiResponse, ListSubgroupsApiResponse, NamespaceApiResponse,
    NodeIdentityApiResponse, PairDeviceCompleteApiResponse, PairDeviceInitApiResponse,
    RemoveGroupMembersApiResponse, ReparentGroupApiResponse, RevokeDeviceApiResponse,
    SetDefaultCapabilitiesApiResponse, SetMemberCapabilitiesApiResponse, SetMetadataApiResponse,
    SetSubgroupVisibilityApiResponse, SyncGroupApiResponse, UpdateMemberRoleApiResponse,
    UpgradeGroupApiResponse,
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
        let _ = table.add_row(vec!["Bytecode ID", &d.bytecode_id]);
        let _ = table.add_row(vec!["Application ID", &d.target_application_id.to_string()]);
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

impl Report for NodeIdentityApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Node Identity").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Account ID", &self.data.account_id]);
        // Absent until this node enrols somewhere. The account above is still
        // real — it is derived from the root — but no device speaks for it yet.
        let _ = table.add_row(vec![
            "Device ID",
            self.data
                .device_id
                .as_deref()
                .unwrap_or("none - joining a namespace enrols one"),
        ]);
        let _ = table.add_row(vec!["Signing key", &self.data.public_key]);
        // The remaining two are what OFFLINE commands consume, and neither was
        // rendered here: the API carried them and `account show` did not print
        // them, so an operator holding a cold account root could read this table
        // and still not have the inputs to certify or pair anything.
        let _ = table.add_row(vec![
            "Agreement key",
            self.data
                .device_agreement_key
                .as_deref()
                .unwrap_or("none - joining a namespace enrols one"),
        ]);
        let _ = table.add_row(vec!["Account root key", &self.data.account_root_public_key]);
        // The two flags an operator needs to tell a half-finished pairing from a
        // delegate device: both are false between pair-init and pair-complete.
        let _ = table.add_row(vec![
            "Holds account root",
            if self.data.holds_account_root {
                "yes"
            } else {
                "no"
            },
        ]);
        let _ = table.add_row(vec![
            "Device certified",
            if self.data.device_certified {
                "yes"
            } else {
                "no - pair-complete on the account holder certifies it"
            },
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
        // Printed outside the table on purpose: it is long, and it has to survive
        // a copy-paste to the device it certifies — which needs it to present
        // itself and cannot read it off the DAG, being a member of nothing.
        println!();
        println!("Device credential (hand this to the paired device):");
        println!("{}", self.data.credential);
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
        // One row per namespace, because a device belongs to an account rather
        // than to a scope and the revocation follows it everywhere. Without the
        // rotation the device stops writing there but keeps the key it already
        // holds — a silent reader — so say so per namespace rather than report a
        // bare success.
        for outcome in &self.data.revoked_in {
            let _ = table.add_row(vec![
                format!("Revoked in {}", outcome.namespace_id),
                if outcome.key_rotated {
                    "key rotated - it can no longer read either".to_owned()
                } else {
                    "key NOT rotated - it can still READ until an admin rotates".to_owned()
                },
            ]);
        }
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
        let _ = table.add_row(vec!["Bytecode ID", &self.bytecode_id]);
        let _ = table.add_row(vec!["Application ID", &self.target_application_id]);
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
            Cell::new("Members").fg(Color::Blue),
            Cell::new("Contexts").fg(Color::Blue),
            Cell::new("Subgroups").fg(Color::Blue),
            Cell::new("Name").fg(Color::Blue),
        ]);
        for ns in &self.data {
            let _ = table.add_row(vec![
                ns.namespace_id.clone(),
                ns.target_application_id.clone(),
                ns.member_count.to_string(),
                ns.context_count.to_string(),
                ns.subgroup_count.to_string(),
                ns.name.clone().unwrap_or_else(|| "-".to_owned()),
            ]);
        }
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

impl Report for JoinNamespaceApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Namespace Joined").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Namespace ID", &self.data.namespace_id]);
        let _ = table.add_row(vec![
            "Member Identity",
            &self.data.member_identity.to_string(),
        ]);
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
        if let Some(total) = d.local_contexts_total {
            let _ = table.add_row(vec!["Local Contexts Total", &total.to_string()]);
        }
        if let Some(swapped) = d.local_contexts_swapped {
            let _ = table.add_row(vec!["Local Contexts Swapped", &swapped.to_string()]);
        }
        if let Some(failed) = d.local_contexts_failed {
            let _ = table.add_row(vec!["Local Contexts Failed", &failed.to_string()]);
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
                if let Some(total) = upgrade.local_contexts_total {
                    let _ = table.add_row(vec!["Local Contexts Total", &total.to_string()]);
                }
                if let Some(swapped) = upgrade.local_contexts_swapped {
                    let _ = table.add_row(vec!["Local Contexts Swapped", &swapped.to_string()]);
                }
                if let Some(failed) = upgrade.local_contexts_failed {
                    let _ = table.add_row(vec!["Local Contexts Failed", &failed.to_string()]);
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
        let _ = table.add_row(vec!["Bytecode ID", &d.bytecode_id]);
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
