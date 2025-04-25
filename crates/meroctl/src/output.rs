use clap::ValueEnum;
use color_eyre::owo_colors::OwoColorize;
use comfy_table::{Cell, Table};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Format {
    Json,
    #[default]
    PlainText,
    PrettyText,
}

#[derive(Debug, Default)]
pub struct Output {
    format: Format,
}

pub trait Report {
    fn report(&self);

    fn pretty_report(&self) {
        self.report();
    }
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
            Format::PlainText => value.report(),
            Format::PrettyText => value.pretty_report(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct InfoLine<'a>(pub &'a str);

impl Report for InfoLine<'_> {
    fn report(&self) {
        println!("{} {}", "[INFO]".green(), self.0);
    }

    fn pretty_report(&self) {
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
        println!("{} {}", "[ERROR]".red(), self.0);
    }

    fn pretty_report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("ERROR").fg(comfy_table::Color::Red)]);
        let _ = table.add_row(vec![self.0]);
        println!("{table}");
    }
}
