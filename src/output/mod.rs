pub mod human;
pub mod json;

use serde::Serialize;

/// Output format selector
#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
    Json,
    Human,
}

/// Print a value in the selected format
pub fn print_output<T: Serialize + HumanReadable>(
    value: &T,
    format: OutputFormat,
) -> anyhow::Result<()> {
    match format {
        OutputFormat::Json => json::print_json(value),
        OutputFormat::Human => {
            value.print_human();
            Ok(())
        }
    }
}

/// Trait for types that can render as human-readable tables
pub trait HumanReadable {
    fn print_human(&self);
}
