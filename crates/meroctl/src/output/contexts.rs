use calimero_server_primitives::admin::{
    CreateContextResponse, DeleteContextResponse, GenerateContextIdentityResponse,
    GetContextIdentitiesResponse, GetContextResponse, GetContextStorageResponse,
    GetContextsResponse, GetPeersCountResponse, PerformIntentApiResponse, SyncContextResponse,
    UpdateContextApplicationResponse,
};
use calimero_server_primitives::jsonrpc::Response;
use comfy_table::{Cell, Color, Table};

use super::Report;

impl Report for CreateContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context Created").fg(Color::Green),
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
            Cell::new("Name").fg(Color::Blue),
            Cell::new("Application ID").fg(Color::Blue),
            Cell::new("Version").fg(Color::Blue),
            Cell::new("Root Hash").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            context.id.to_string(),
            context.name.clone().unwrap_or_default(),
            context.application_id.to_string(),
            context.application_version.clone().unwrap_or_default(),
            format!("{:?}", context.root_hash),
        ]);

        println!("{table}");
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
                Cell::new("Name").fg(Color::Blue),
                Cell::new("Application ID").fg(Color::Blue),
                Cell::new("Version").fg(Color::Blue),
                Cell::new("Root Hash").fg(Color::Blue),
            ]);

            for entry in &self.data.contexts {
                let _ = table.add_row(vec![
                    entry.context.id.to_string(),
                    entry.context.name.clone().unwrap_or_default(),
                    entry.context.application_id.to_string(),
                    entry
                        .context
                        .application_version
                        .clone()
                        .unwrap_or_default(),
                    format!("{:?}", entry.context.root_hash),
                ]);
            }

            println!("{table}");
        }
    }
}

impl Report for PerformIntentApiResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Intent Performed").fg(Color::Green)]);
        // The node ran it and published the result attributed to the author. What
        // a caller most wants next is the method's own answer.
        let _ = table.add_row(vec![
            "Returned",
            &self
                .returns
                .as_ref()
                .map_or_else(|| "(nothing)".to_owned(), ToString::to_string),
        ]);
        if let Some(delta) = &self.delta_id {
            let _ = table.add_row(vec!["Delta", delta]);
        }
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
