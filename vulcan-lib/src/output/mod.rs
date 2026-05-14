//! Output formatting — JSON envelopes and human-readable tables.

mod json;
pub mod table;

pub use json::{ErrorEnvelope, SuccessEnvelope};
pub use table::render_table;

use crate::error::VulcanError;
use serde::Serialize;

/// Output format selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Json,
    #[default]
    Table,
}

/// Render a successful result to stdout in the requested format.
pub fn render_success<T: Serialize + TableRenderable>(
    format: OutputFormat,
    data: &T,
    meta: serde_json::Value,
) {
    match format {
        OutputFormat::Json => {
            let envelope = SuccessEnvelope::new(data, meta);
            // unwrap is safe: we control the types and they are Serialize
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        }
        OutputFormat::Table => {
            data.render_table();
        }
    }
}

/// Render an error to stdout in the requested format.
pub fn render_error(format: OutputFormat, error: &VulcanError) {
    match format {
        OutputFormat::Json => {
            let envelope = ErrorEnvelope::from_error(error);
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        }
        OutputFormat::Table => {
            eprintln!("Error: {}", error);
        }
    }
}

/// Trait for types that can render themselves as a table.
pub trait TableRenderable {
    fn render_table(&self);
}
