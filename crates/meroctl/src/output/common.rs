use color_eyre::owo_colors::OwoColorize;
use serde::Serialize;

use super::Report;

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
