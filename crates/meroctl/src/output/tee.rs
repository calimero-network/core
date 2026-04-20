use calimero_server_primitives::admin::FleetJoinResponse;
use comfy_table::{Cell, Color, Table};

use super::Report;

impl Report for FleetJoinResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Fleet Join").fg(Color::Green),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Status", &self.status]);
        let _ = table.add_row(vec!["Admitted", &self.admitted.to_string()]);
        let _ = table.add_row(vec![
            "Auto-follow enabled",
            &self.auto_follow_enabled.to_string(),
        ]);
        let _ = table.add_row(vec!["Group ID", &self.group_id]);
        let _ = table.add_row(vec!["Namespace ID", &self.namespace_id]);
        let _ = table.add_row(vec!["Public Key", &self.public_key]);
        let _ = table.add_row(vec![
            "Contexts Joined",
            &self.contexts_joined.len().to_string(),
        ]);
        println!("{table}");
        for (idx, ctx) in self.contexts_joined.iter().enumerate() {
            println!("  [{idx}] {ctx}");
        }
    }
}
