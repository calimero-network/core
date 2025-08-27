// Import types needed for Report implementations
use calimero_primitives::application::Application;
use calimero_server_primitives::admin::{
    CreateAliasResponse, CreateContextResponse, DeleteAliasResponse, DeleteContextResponse,
    GenerateContextIdentityResponse, GetApplicationResponse, GetContextClientKeysResponse,
    GetContextIdentitiesResponse, GetContextResponse, GetContextStorageResponse, GetContextUsersResponse,
    GetContextsResponse, GetPeersCountResponse, GetProposalApproversResponse, GetProposalResponse,
    GetProposalsResponse, GrantPermissionResponse, InstallApplicationResponse, InviteToContextResponse,
    JoinContextResponse, ListAliasesResponse, ListApplicationsResponse, LookupAliasResponse,
    RevokePermissionResponse, SyncContextResponse, UninstallApplicationResponse,
    UpdateContextApplicationResponse,
};
use calimero_server_primitives::jsonrpc::Response;
use clap::ValueEnum;
use color_eyre::owo_colors::OwoColorize;
use comfy_table::{Cell, Color, Table};
use serde::Serialize;

// Import the response types from client
use crate::client::{
    BlobDeleteResponse, BlobInfoResponse, BlobListResponse, ResolveResponse, ResolveResponseValue,
};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Format {
    Json,
    #[default]
    Human,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Output {
    format: Format,
}

pub trait Report {
    fn report(&self);
}

impl Output {
    pub const fn new(output_type: Format) -> Self {
        Self {
            format: output_type,
        }
    }

    pub fn write<T: Serialize + Report>(&self, value: &T) {
        match self.format {
            Format::Json => match serde_json::to_string(&value) {
                Ok(json) => println!("{json}"),
                Err(err) => eprintln!("Failed to serialize to JSON: {err}"),
            },
            Format::Human => value.report(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct InfoLine<'a>(pub &'a str);

impl Report for InfoLine<'_> {
    fn report(&self) {
        println!("{} {}", "[INFO]".green(), self.0);
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorLine<'a>(pub &'a str);

impl Report for ErrorLine<'_> {
    fn report(&self) {
        println!("{} {}", "[ERROR]".red(), self.0);
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WarnLine<'a>(pub &'a str);

impl Report for WarnLine<'_> {
    fn report(&self) {
        println!("{} {}", "[WARN]".yellow(), self.0);
    }
}

// Blob-related Report implementations
impl Report for BlobDeleteResponse {
    fn report(&self) {
        if self.deleted {
            println!("Successfully deleted blob '{}'", self.blob_id);
        } else {
            println!(
                "Failed to delete blob '{}' (blob may not exist)",
                self.blob_id
            );
        }
    }
}

impl Report for BlobListResponse {
    fn report(&self) {
        if self.data.blobs.is_empty() {
            println!("No blobs found");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Blob ID").fg(Color::Blue),
                Cell::new("Size").fg(Color::Blue),
            ]);
            for blob in &self.data.blobs {
                let _ = table.add_row(vec![
                    blob.blob_id.to_string(),
                    format!("{} bytes", blob.size),
                ]);
            }
            println!("{table}");
        }
    }
}

impl Report for BlobInfoResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Blob ID").fg(Color::Blue),
            Cell::new("Size (bytes)").fg(Color::Blue),
            Cell::new("MIME Type").fg(Color::Blue),
            Cell::new("Hash").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            &self.data.blob_id.to_string(),
            &self.data.size.to_string(),
            &self.data.mime_type,
            &hex::encode(self.data.hash),
        ]);

        println!("{table}");
    }
}

// Alias-related Report implementations
impl Report for CreateAliasResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Created").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully created alias"]);
        println!("{table}");
    }
}

impl Report for DeleteAliasResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Deleted").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully deleted alias"]);
        println!("{table}");
    }
}

impl<T: std::fmt::Display> Report for ListAliasesResponse<T> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Value").fg(Color::Blue),
            Cell::new("Alias").fg(Color::Blue),
        ]);

        for (alias, value) in &self.data {
            let _ = table.add_row(vec![
                Cell::new(value.to_string()),
                Cell::new(alias.as_str()),
            ]);
        }

        println!("{table}");
    }
}

impl<T: std::fmt::Display> Report for LookupAliasResponse<T> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Lookup").fg(Color::Blue)]);

        match &self.data.value {
            Some(value) => {
                let _ = table.add_row(vec!["Status", "Found"]);
                let _ = table.add_row(vec!["Value", &value.to_string()]);
            }
            None => {
                let _ = table.add_row(vec!["Status", "Not Found"]);
            }
        }
        println!("{table}");
    }
}

impl<T: std::fmt::Display> Report for ResolveResponse<T> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Alias Resolution").fg(Color::Blue)]);
        let _ = table.add_row(vec!["Alias", self.alias().as_str()]);

        match self.value_enum() {
            Some(ResolveResponseValue::Lookup(value)) => {
                let _ = table.add_row(vec!["Type", "Lookup"]);
                value.report();
            }
            Some(ResolveResponseValue::Parsed(value)) => {
                let _ = table.add_row(vec!["Type", "Direct"]);
                let _ = table.add_row(vec!["Value", &value.to_string()]);
            }
            None => {
                let _ = table.add_row(vec!["Status", "Not Resolved"]);
            }
        }
        println!("{table}");
    }
}

// Application-related Report implementations
impl Report for Application {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("ID").fg(Color::Blue),
            Cell::new("Name").fg(Color::Blue),
            Cell::new("Version").fg(Color::Blue),
            Cell::new("Description").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            &self.id.to_string(),
            &self.source.to_string(),
            &self.size.to_string(),
            &format!("Blob: {}", self.blob.bytecode),
        ]);

        println!("{table}");
    }
}

impl Report for GetApplicationResponse {
    fn report(&self) {
        if let Some(app) = &self.data.application {
            app.report();
        } else {
            println!("No application found");
        }
    }
}

impl Report for InstallApplicationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Application Installed").fg(Color::Green)]);
        let _ = table.add_row(vec![
            format!("Successfully installed application '{}'", self.data.application_id),
        ]);

        println!("{table}");
    }
}

impl Report for ListApplicationsResponse {
    fn report(&self) {
        if self.data.apps.is_empty() {
            println!("No applications found");
        } else {
            let mut table = Table::new();
            let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
            let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

            let _ = table.set_header(vec![
                Cell::new("ID").fg(Color::Blue),
                Cell::new("Source").fg(Color::Blue),
                Cell::new("Size").fg(Color::Blue),
                Cell::new("Blob").fg(Color::Blue),
            ]);

            for app in &self.data.apps {
                let _ = table.add_row(vec![
                    &app.id.to_string(),
                    &app.source.to_string(),
                    &app.size.to_string(),
                    &format!("Blob: {}", app.blob.bytecode),
                ]);
            }

            println!("{table}");
        }
    }
}

impl Report for UninstallApplicationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Application Uninstalled").fg(Color::Green)]);
        let _ = table.add_row(vec![
            format!("Successfully uninstalled application '{}'", self.data.application_id),
        ]);

        println!("{table}");
    }
}

// Context-related Report implementations
impl Report for CreateContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Context Created").fg(Color::Green)]);
        let _ = table.add_row(vec![
            format!("Successfully created context '{}'", self.data.context_id),
        ]);

        println!("{table}");
    }
}

impl Report for DeleteContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Context Deleted").fg(Color::Green)]);
        let _ = table.add_row(vec![
            format!("Successfully deleted context (deleted: {})", self.data.is_deleted),
        ]);

        println!("{table}");
    }
}

impl Report for GetContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("Context ID").fg(Color::Blue),
            Cell::new("Application ID").fg(Color::Blue),
            Cell::new("Executor ID").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            &self.data.id.to_string(),
            &self.data.application_id.to_string(),
            &self.data.root_hash.to_string(),
        ]);

        println!("{table}");
    }
}

impl Report for GetContextUsersResponse {
    fn report(&self) {
        if self.data.context_users.is_empty() {
            println!("No users found");
        } else {
            let mut table = Table::new();
            let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
            let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

            let _ = table.set_header(vec![Cell::new("User ID").fg(Color::Blue)]);

            for user in &self.data.context_users {
                let _ = table.add_row(vec![&user.user_id]);
            }

            println!("{table}");
        }
    }
}

impl Report for GetContextClientKeysResponse {
    fn report(&self) {
        if self.data.client_keys.is_empty() {
            println!("No client keys found");
        } else {
            let mut table = Table::new();
            let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
            let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

            let _ = table.set_header(vec![
                Cell::new("Client ID").fg(Color::Blue),
                Cell::new("Public Key").fg(Color::Blue),
            ]);

            for client_key in &self.data.client_keys {
                let _ = table.add_row(vec![
                    &client_key.context_id.map(|id| id.to_string()).unwrap_or_else(|| "N/A".to_owned()),
                    &client_key.signing_key,
                ]);
            }

            println!("{table}");
        }
    }
}

impl Report for GetContextStorageResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("Storage Type").fg(Color::Blue),
            Cell::new("Storage ID").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            "Size",
            &self.data.size_in_bytes.to_string(),
        ]);

        println!("{table}");
    }
}

impl Report for GetContextIdentitiesResponse {
    fn report(&self) {
        if self.data.identities.is_empty() {
            println!("No identities found");
        } else {
            let mut table = Table::new();
            let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
            let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

            let _ = table.set_header(vec![
                Cell::new("Identity ID").fg(Color::Blue),
                Cell::new("Public Key").fg(Color::Blue),
            ]);

            for identity in &self.data.identities {
                let _ = table.add_row(vec![
                    "Identity",
                    &hex::encode(identity.as_ref()),
                ]);
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
            let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
            let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

            let _ = table.set_header(vec![
                Cell::new("Context ID").fg(Color::Blue),
                Cell::new("Application ID").fg(Color::Blue),
                Cell::new("Executor ID").fg(Color::Blue),
            ]);

            for context in &self.data.contexts {
                let _ = table.add_row(vec![
                    &context.id.to_string(),
                    &context.application_id.to_string(),
                    &context.root_hash.to_string(),
                ]);
            }

            println!("{table}");
        }
    }
}

impl Report for GrantPermissionResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Permission Granted").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully granted permission"]);

        println!("{table}");
    }
}

impl Report for InviteToContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Invitation Created").fg(Color::Green)]);
        let _ = table.add_row(vec![
            format!("Successfully created invitation for context"),
        ]);

        println!("{table}");
    }
}

impl Report for JoinContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Context Joined").fg(Color::Green)]);
        let _ = table.add_row(vec![
            format!("Successfully joined context '{}'", self.data.as_ref().map(|d| d.context_id).unwrap_or_default()),
        ]);

        println!("{table}");
    }
}

impl Report for RevokePermissionResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Permission Revoked").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully revoked permission"]);

        println!("{table}");
    }
}

impl Report for SyncContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Context Synced").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully synced context"]);

        println!("{table}");
    }
}

impl Report for UpdateContextApplicationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Context Updated").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully updated context application"]);

        println!("{table}");
    }
}

// Define the ProposalDetailsResponse struct since it's not in the admin module
#[derive(Debug, Serialize)]
pub struct ProposalDetailsResponse {
    pub proposal: GetProposalResponse,
    pub approvers: GetProposalApproversResponse,
}

// Context proposals Report implementations

impl Report for ProposalDetailsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("Proposal ID").fg(Color::Blue),
            Cell::new("Author").fg(Color::Blue),
            Cell::new("Actions").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            &self.proposal.data.id.to_string(),
            &self.proposal.data.author_id.to_string(),
            &self.proposal.data.actions.len().to_string(),
        ]);

        println!("{table}");

        if !self.approvers.data.is_empty() {
            println!("\nApprovers:");
            let mut approver_table = Table::new();
            let _ = approver_table.load_preset(comfy_table::presets::UTF8_FULL);
            let _ = approver_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

            let _ = approver_table.set_header(vec![Cell::new("Approver ID").fg(Color::Blue)]);

            for approver in &self.approvers.data {
                let _ = approver_table.add_row(vec![format!("{}", approver)]);
            }

            println!("{approver_table}");
        }
    }
}

impl Report for GetProposalResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("ID").fg(Color::Blue),
            Cell::new("Author").fg(Color::Blue),
            Cell::new("Actions").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            format!("{}", self.data.id),
            format!("{}", self.data.author_id),
            format!("{}", self.data.actions.len()),
        ]);

        println!("{table}");
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Approver ID").fg(Color::Blue)]);

        if self.data.is_empty() {
            let _ = table.add_row(vec!["No approvers found"]);
        } else {
            for approver in &self.data {
                let _ = table.add_row(vec![format!("{}", approver)]);
            }
        }

        println!("{table}");
    }
}

impl Report for GetProposalsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("ID").fg(Color::Blue),
            Cell::new("Author").fg(Color::Blue),
            Cell::new("Actions").fg(Color::Blue),
        ]);

        if self.data.is_empty() {
            let _ = table.add_row(vec!["No proposals found", "", ""]);
        } else {
            for proposal in &self.data {
                let _ = table.add_row(vec![
                    format!("{}", proposal.id),
                    format!("{}", proposal.author_id),
                    format!("{}", proposal.actions.len()),
                ]);
            }
        }

        println!("{table}");
    }
}

// Context identity Report implementations
impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("Identity Generated").fg(Color::Green),
            Cell::new("Identity ID").fg(Color::Blue),
            Cell::new("Public Key").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            "Success",
            "Generated Identity",
            &hex::encode(self.data.public_key.as_ref()),
        ]);

        println!("{table}");
    }
}

// Network Report implementations
impl Report for GetPeersCountResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("Peers Count").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![&self.count.to_string()]);

        println!("{table}");
    }
}

// RPC Report implementations
impl Report for Response {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("Status").fg(Color::Blue),
            Cell::new("Result").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            "Success",
            &serde_json::to_string_pretty(&self.body).unwrap_or_else(|_| "N/A".to_owned()),
        ]);

        println!("{table}");
    }
}




