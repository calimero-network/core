use calimero_primitives::application::Application;
use calimero_server_primitives::admin::{
    GetApplicationResponse, GetLatestVersionResponse, InstallApplicationResponse,
    ListApplicationsResponse, ListPackagesResponse, ListVersionsResponse,
    UninstallApplicationResponse,
};
use comfy_table::{Cell, Color, Table};

use super::Report;

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
        let _ = table.add_row(vec![format!(
            "Successfully installed application '{}'",
            self.data.application_id
        )]);

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
        let _ = table.add_row(vec![format!(
            "Successfully uninstalled application '{}'",
            self.data.application_id
        )]);

        println!("{table}");
    }
}

impl Report for ListPackagesResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Package").fg(Color::Blue)]);

        for package in &self.packages {
            let _ = table.add_row(vec![package.clone()]);
        }

        println!("{table}");
    }
}

impl Report for ListVersionsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Version").fg(Color::Blue)]);

        for version in &self.versions {
            let _ = table.add_row(vec![version.clone()]);
        }

        println!("{table}");
    }
}

impl Report for GetLatestVersionResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Application ID").fg(Color::Blue)]);

        match &self.application_id {
            Some(id) => {
                let _ = table.add_row(vec![id.to_string()]);
            }
            None => {
                let _ = table.add_row(vec!["No latest version found".to_string()]);
            }
        }

        println!("{table}");
    }
}
