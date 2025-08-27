use comfy_table::{Cell, Color, Table};

use super::Report;
use calimero_server_primitives::admin::{CreateAliasResponse, DeleteAliasResponse, ListAliasesResponse, LookupAliasResponse};
use crate::client::{ResolveResponse, ResolveResponseValue};

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
