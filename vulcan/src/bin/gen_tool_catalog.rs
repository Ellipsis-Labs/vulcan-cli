//! Regenerate `agents/tool-catalog.json` from the MCP tool registry.
//!
//! Usage: `cargo run -p vulcan --bin gen_tool_catalog > agents/tool-catalog.json`
//!
//! A unit test in `vulcan-lib/src/mcp/catalog_gen.rs` enforces that the
//! checked-in file matches this generator's output — if it fails after a
//! registry edit, rerun this binary and commit the regenerated JSON.

fn main() {
    let catalog = vulcan_lib::mcp::catalog_gen::generate();
    let json =
        serde_json::to_string_pretty(&catalog).expect("catalog serialization should not fail");
    println!("{json}");
}
