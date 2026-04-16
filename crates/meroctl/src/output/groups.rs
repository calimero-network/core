use calimero_server_primitives::admin::{
    AddGroupMembersApiResponse, CreateGroupApiResponse, CreateGroupInvitationApiResponse,
    CreateNamespaceApiResponse, DeleteGroupApiResponse, DeleteNamespaceApiResponse,
    DetachContextFromGroupApiResponse, GetGroupUpgradeStatusApiResponse,
    GetMemberCapabilitiesApiResponse, GroupInfoApiResponse, JoinContextApiResponse,
    JoinGroupApiResponse, ListGroupContextsApiResponse, ListGroupMembersApiResponse,
    ListNamespaceGroupsApiResponse, ListNamespacesApiResponse, ListSubgroupsApiResponse,
    NamespaceApiResponse, NamespaceIdentityApiResponse, NestGroupApiResponse,
    RegisterGroupSigningKeyApiResponse, RemoveGroupMembersApiResponse,
    SetDefaultCapabilitiesApiResponse, SetDefaultVisibilityApiResponse,
    SetMemberCapabilitiesApiResponse, SyncGroupApiResponse, UnnestGroupApiResponse,
    UpdateGroupSettingsApiResponse, UpdateMemberRoleApiResponse, UpgradeGroupApiResponse,
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
        if let Some(ref upgrade) = d.active_upgrade {
            let _ = table.add_row(vec!["Active Upgrade Status", &upgrade.status]);
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
        let _ = table.add_row(vec!["App Key", &self.app_key]);
        let _ = table.add_row(vec!["Application ID", &self.target_application_id]);
        let _ = table.add_row(vec!["Upgrade Policy", &self.upgrade_policy]);
        let _ = table.add_row(vec!["Created At", &self.created_at.to_string()]);
        let _ = table.add_row(vec!["Members", &self.member_count.to_string()]);
        let _ = table.add_row(vec!["Contexts", &self.context_count.to_string()]);
        let _ = table.add_row(vec!["Subgroups", &self.subgroup_count.to_string()]);
        if let Some(ref alias) = self.alias {
            let _ = table.add_row(vec!["Alias", alias]);
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
            Cell::new("Alias").fg(Color::Blue),
        ]);
        for ns in &self.data {
            let _ = table.add_row(vec![
                ns.namespace_id.clone(),
                ns.target_application_id.clone(),
                ns.upgrade_policy.clone(),
                ns.member_count.to_string(),
                ns.context_count.to_string(),
                ns.subgroup_count.to_string(),
                ns.alias.clone().unwrap_or_else(|| "-".to_owned()),
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
            Cell::new("Alias").fg(Color::Blue),
        ]);
        for group in &self.data {
            let _ = table.add_row(vec![
                group.group_id.clone(),
                group.alias.clone().unwrap_or_else(|| "-".to_owned()),
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
            Cell::new("Alias").fg(Color::Blue),
        ]);
        for group in &self.subgroups {
            let _ = table.add_row(vec![
                group.group_id.clone(),
                group.alias.clone().unwrap_or_else(|| "-".to_owned()),
            ]);
        }
        println!("{table}");
    }
}

impl Report for NestGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Group Nested").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully nested group"]);
        println!("{table}");
    }
}

impl Report for UnnestGroupApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Group Unnested").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully unnested group"]);
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
        if self.data.is_empty() {
            println!("No members found in group");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Identity").fg(Color::Blue),
                Cell::new("Role").fg(Color::Blue),
            ]);
            for member in &self.data {
                let _ = table.add_row(vec![
                    member.identity.to_string(),
                    format!("{:?}", member.role),
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
                Cell::new("Alias").fg(Color::Blue),
            ]);
            for entry in &self.data {
                let alias = entry.alias.as_deref().unwrap_or("-");
                let _ = table.add_row(vec![entry.context_id.clone(), alias.to_owned()]);
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
            if caps & (1 << 0) != 0 { "yes" } else { "no" }.to_owned(),
        ]);
        let _ = table.add_row(vec![
            "CAN_INVITE_MEMBERS".to_owned(),
            if caps & (1 << 1) != 0 { "yes" } else { "no" }.to_owned(),
        ]);
        let _ = table.add_row(vec![
            "CAN_JOIN_OPEN_CONTEXTS".to_owned(),
            if caps & (1 << 2) != 0 { "yes" } else { "no" }.to_owned(),
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

impl Report for SetDefaultVisibilityApiResponse {
    fn report(&self) {
        println!(
            "{}",
            "Default context visibility updated successfully".green()
        );
    }
}
