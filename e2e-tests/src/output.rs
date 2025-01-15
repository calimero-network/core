use clap::ValueEnum;
use eyre::{Ok, Result as EyreResult};
use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct OutputWriter {
    format: OutputFormat,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum OutputFormat {
    Markdown,
    #[default]
    PlainText,
}

impl OutputWriter {
    pub const fn new(format: OutputFormat) -> Self {
        Self { format }
    }

    pub fn write_str(self, line: &str) {
        match self.format {
            OutputFormat::Markdown => println!("{line}  "),
            OutputFormat::PlainText => println!("{line}"),
        }
    }

    pub fn write_header(self, header: &str, level: usize) {
        match self.format {
            OutputFormat::Markdown => println!("{} {}  ", "#".repeat(level), header),
            OutputFormat::PlainText => {
                println!(
                    "{}{}{}",
                    "-".repeat(level * 5),
                    header,
                    "-".repeat(level * 5),
                );
            }
        }
    }

    pub fn write_json<T>(self, json: &T) -> EyreResult<()>
    where
        T: ?Sized + Serialize,
    {
        match self.format {
            OutputFormat::Markdown => {
                println!("```json\n{}\n```", serde_json::to_string_pretty(json)?);
            }
            OutputFormat::PlainText => {
                println!("{}", serde_json::to_string(json)?);
            }
        }

        Ok(())
    }
}
