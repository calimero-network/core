use clap::ValueEnum;
use color_eyre::owo_colors::OwoColorize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Format {
    Json,
    #[default]
    PlainText,
}

#[derive(Debug, Default)]
pub struct Output {
    format: Format,
}

pub trait Report {
    fn report(&self);

    // New method for pretty printing
    fn pretty_report(&self) {
        self.report();
    }
}

impl PrettyTable {
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|s| s.to_string()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn add_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    pub fn print(&self) {
        if self.headers.is_empty() || self.rows.is_empty() {
            return;
        }

        // Calculate column widths
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();

        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }

        // Print header
        let header = self
            .headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!(" {:width$} ", h.bold().blue(), width = widths[i]))
            .collect::<Vec<_>>()
            .join("│");

        println!("{}", header);
        println!("{}", "─".repeat(header.len()));

        // Print rows
        for row in &self.rows {
            let row_str = row
                .iter()
                .enumerate()
                .map(|(i, cell)| format!(" {:width$} ", cell, width = widths[i]))
                .collect::<Vec<_>>()
                .join("│");

            println!("{}", row_str);
        }
    }
}

// Implement PrettyTable for Report where applicable
impl Report for PrettyTable {
    fn report(&self) {
        self.print();
    }
}

pub struct PrettyTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
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
            Format::PlainText => value.pretty_report(),
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
