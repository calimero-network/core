use clap::ValueEnum;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, ValueEnum, Serialize)]
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
}

impl Output {
    pub fn new(output_type: Format) -> Self {
        Output {
            format: output_type,
        }
    }

    pub fn write<T: Serialize + Report>(&self, value: &T) {
        match self.format {
            Format::Json => match serde_json::to_string(&value) {
                Ok(json) => println!("{}", json),
                Err(e) => eprintln!("Failed to serialize to JSON: {}", e),
            },
            Format::PlainText => value.report(),
        }
    }
}
