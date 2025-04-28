use clap::ValueEnum;
use comfy_table::{Cell, Table};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Format {
    Json,
    #[default]
    Human,
}

#[derive(Debug, Default)]
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
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("INFO").fg(comfy_table::Color::Green)]);
        let _ = table.add_row(vec![self.0]);
        println!("{table}");
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorLine<'a>(pub &'a str);

impl Report for ErrorLine<'_> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("ERROR").fg(comfy_table::Color::Red)]);
        let _ = table.add_row(vec![self.0]);
        println!("{table}");
    }
}
