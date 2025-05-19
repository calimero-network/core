use clap::ValueEnum;
use color_eyre::owo_colors::OwoColorize;
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
