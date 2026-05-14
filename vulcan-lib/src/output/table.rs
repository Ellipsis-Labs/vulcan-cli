//! Table rendering helpers using comfy-table.

use comfy_table::{presets::UTF8_FULL, Table};

/// Build a table with standard Vulcan styling.
pub fn render_table(headers: &[&str], rows: Vec<Vec<String>>) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(headers);
    for row in rows {
        table.add_row(row);
    }
    println!("{table}");
}
