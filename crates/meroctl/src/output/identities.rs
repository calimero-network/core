use calimero_server_primitives::admin::{
    ExportIdentityResponse, GenerateIdentityResponse, ImportIdentityResponse,
    ListIdentitiesResponse, RemoveIdentityResponse,
};
use comfy_table::{Cell, Color, Table};

use super::Report;

impl Report for GenerateIdentityResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Identity Generated").fg(Color::Green),
            Cell::new("Public Key").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec![
            "Successfully generated identity",
            &self.data.public_key.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for ListIdentitiesResponse {
    fn report(&self) {
        if self.data.identities.is_empty() {
            println!("No identities found");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Public Key").fg(Color::Blue),
                Cell::new("Alias").fg(Color::Blue),
            ]);

            for identity in &self.data.identities {
                let _ = table.add_row(vec![
                    identity.public_key.to_string(),
                    identity.alias.as_deref().unwrap_or("None").to_string(),
                ]);
            }

            println!("{table}");
        }
    }
}

impl Report for ExportIdentityResponse {
    fn report(&self) {
        println!("{}", self.data.json);
    }
}

impl Report for ImportIdentityResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Identity Imported").fg(Color::Green),
            Cell::new("Public Key").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec![
            "Successfully imported identity",
            &self.data.public_key.to_string(),
        ]);
        println!("{table}");
    }
}

impl Report for RemoveIdentityResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Identity Removed").fg(Color::Green)]);
        let _ = table.add_row(vec!["Successfully removed identity"]);
        println!("{table}");
    }
}
