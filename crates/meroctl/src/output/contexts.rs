use calimero_server_primitives::admin::{
    CreateContextResponse, DeleteContextResponse, GenerateContextIdentityResponse,
    GetContextClientKeysResponse, GetContextIdentitiesResponse, GetContextResponse,
    GetContextStorageResponse, GetContextUsersResponse, GetContextsResponse, GetPeersCountResponse,
    GrantPermissionResponse, InviteToContextOpenInvitationResponse, InviteToContextResponse,
    JoinContextResponse, RevokePermissionResponse, SyncContextResponse,
    UpdateContextApplicationResponse,
};
use calimero_server_primitives::jsonrpc::Response;
use comfy_table::{Cell, Color, Table};

use super::Report;

impl Report for CreateContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Created").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully created context"]);
        println!("{table}");
    }
}

impl Report for DeleteContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Deleted").fg(Color::Green)]);
        let _ = table.add_row(vec![format!(
            "Successfully deleted context (deleted: {})",
            self.data.is_deleted
        )]);
        println!("{table}");
    }
}

impl Report for GetContextResponse {
    fn report(&self) {
        let context = &self.data;
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context ID").fg(Color::Blue),
            Cell::new("Application ID").fg(Color::Blue),
            Cell::new("Root Hash").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            context.id.to_string(),
            context.application_id.to_string(),
            format!("{:?}", context.root_hash),
        ]);

        println!("{table}");
    }
}

impl Report for GetContextUsersResponse {
    fn report(&self) {
        if self.data.context_users.is_empty() {
            println!("No users found in context");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("User ID").fg(Color::Blue),
                Cell::new("Type").fg(Color::Blue),
            ]);

            for user in &self.data.context_users {
                let _ = table.add_row(vec![format!("{:?}", user), "Context User".to_owned()]);
            }

            println!("{table}");
        }
    }
}

impl Report for GetContextClientKeysResponse {
    fn report(&self) {
        if self.data.client_keys.is_empty() {
            println!("No client keys found in context");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Wallet Type").fg(Color::Blue),
                Cell::new("Signing Key").fg(Color::Blue),
                Cell::new("Created At").fg(Color::Blue),
            ]);

            for key in &self.data.client_keys {
                let _ = table.add_row(vec![
                    format!("{:?}", key.wallet_type),
                    key.signing_key.clone(),
                    key.created_at.to_string(),
                ]);
            }

            println!("{table}");
        }
    }
}

impl Report for GetContextStorageResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Storage Size").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec!["Size in bytes", &self.data.size_in_bytes.to_string()]);

        println!("{table}");
    }
}

impl Report for GetContextIdentitiesResponse {
    fn report(&self) {
        if self.data.identities.is_empty() {
            println!("No identities found in context");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Identity").fg(Color::Blue),
                Cell::new("Type").fg(Color::Blue),
            ]);

            for identity in &self.data.identities {
                let _ = table.add_row(vec![identity.to_string(), "Context Identity".to_owned()]);
            }

            println!("{table}");
        }
    }
}

impl Report for GetContextsResponse {
    fn report(&self) {
        if self.data.contexts.is_empty() {
            println!("No contexts found");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Context ID").fg(Color::Blue),
                Cell::new("Application ID").fg(Color::Blue),
                Cell::new("Root Hash").fg(Color::Blue),
            ]);

            for context in &self.data.contexts {
                let _ = table.add_row(vec![
                    context.id.to_string(),
                    context.application_id.to_string(),
                    format!("{:?}", context.root_hash),
                ]);
            }

            println!("{table}");
        }
    }
}

impl Report for GrantPermissionResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Permissions Granted").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully granted permissions"]);
        println!("{table}");
    }
}

impl Report for InviteToContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Invitation Sent").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully sent invitation"]);
        println!("{table}");
    }
}

impl Report for InviteToContextOpenInvitationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Open Invitation Created").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully created an open invitation"]);
        println!("{table}");
    }
}

impl Report for JoinContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Joined").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully joined context"]);
        println!("{table}");
    }
}

impl Report for RevokePermissionResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Permissions Revoked").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully revoked permissions"]);
        println!("{table}");
    }
}

impl Report for SyncContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Synced").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully synced context"]);
        println!("{table}");
    }
}

impl Report for UpdateContextApplicationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context Application Updated").fg(Color::Green)
        ]);
        let _ = table.add_row(vec!["Successfully updated application"]);
        println!("{table}");
    }
}

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context Identity Generated").fg(Color::Green),
            Cell::new("Public Key").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec![
            "Successfully generated context identity",
            &self.data.public_key.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for GetPeersCountResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Peers Count").fg(Color::Blue),
            Cell::new("Count").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Connected peers", &self.count.to_string()]);
        println!("{table}");
    }
}

impl Report for Response {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Response").fg(Color::Blue),
            Cell::new("Status").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["JSON-RPC Response", "Success"]);
        println!("{table}");
    }
}
