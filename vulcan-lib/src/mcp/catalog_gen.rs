//! Generator for `agents/tool-catalog.json`.
//!
//! The catalog is a machine-readable mirror of [`crate::mcp::registry::TOOLS`]
//! plus per-tool CLI metadata (command, example, auth flag) and a few static
//! top-level fields (schema version, group descriptions). The checked-in
//! `agents/tool-catalog.json` is the artifact agents fetch via the
//! `vulcan://agents/tool-catalog` MCP resource; this generator is the source
//! of truth and a unit test enforces that the checked-in file matches.
//!
//! Regenerate with `cargo run -p vulcan --bin gen_tool_catalog > agents/tool-catalog.json`.

use crate::mcp::registry::{ToolDef, TOOLS};
use serde_json::{json, Map, Value};

/// Static top-level group descriptions. The order here is the order the
/// catalog publishes them in.
const GROUPS: &[(&str, &str)] = &[
    ("market", "Market data: prices, orderbooks, candles, funding rates"),
    ("trade", "Order management: place, cancel, TP/SL"),
    (
        "position",
        "Position management: view, close, reduce, bracket orders",
    ),
    (
        "margin",
        "Collateral management: deposit, withdraw, transfer, leverage tiers",
    ),
    (
        "history",
        "Phoenix/Rise trader history: fills, orders, collateral, funding, PnL",
    ),
    ("status", "Health check, connectivity, and CLI update detection"),
    ("wallet", "Wallet creation, address, listing, and balance"),
    ("account", "Account info and registration"),
    ("auth", "Phoenix API wallet-session authentication"),
    ("portfolio", "Combined portfolio snapshot in a single fetch"),
    (
        "paper",
        "Local paper trading simulation with live prices and no real funds",
    ),
    (
        "strategy",
        "Curated Vulcan-owned strategy runners with tick logs and reports",
    ),
    (
        "ta",
        "Technical analysis: indicators (SMA, EMA, RSI, MACD, BBands, ATR, VWAP, ADX, Stoch) and trigger evaluation over candle history",
    ),
];

/// Build the full catalog as a [`Value`]. Field order is preserved thanks to
/// `serde_json`'s `preserve_order` feature.
pub fn generate() -> Value {
    let mut groups = Map::new();
    for (name, description) in GROUPS {
        groups.insert(
            (*name).to_string(),
            Value::String((*description).to_string()),
        );
    }
    let commands: Vec<Value> = TOOLS.iter().map(tool_entry).collect();
    json!({
        "schema_version": "1.0.0",
        "cli_version": env!("CARGO_PKG_VERSION"),
        "description": format!(
            "Machine-readable command catalog for vulcan. {} tools across {} groups.",
            TOOLS.len(),
            GROUPS.len(),
        ),
        "groups": Value::Object(groups),
        "commands": commands,
    })
}

fn tool_entry(t: &ToolDef) -> Value {
    let schema = (t.schema)();
    json!({
        "name": t.name,
        "command": t.command,
        "group": t.group,
        "description": t.description,
        "auth_required": t.auth_required,
        "dangerous": t.dangerous,
        "parameters": parameters_from_schema(&schema),
        "example": t.example,
    })
}

/// Flatten a JSON Schema into the catalog's parameter shape:
/// `{ name, type, required, [default], [description] }`.
fn parameters_from_schema(schema: &Value) -> Vec<Value> {
    let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    properties
        .iter()
        .map(|(name, spec)| parameter_entry(name, spec, required.contains(&name.as_str())))
        .collect()
}

fn parameter_entry(name: &str, spec: &Value, required: bool) -> Value {
    let mut entry = Map::new();
    entry.insert("name".into(), Value::String(name.to_string()));
    entry.insert("type".into(), Value::String(parameter_type(spec)));
    entry.insert("required".into(), Value::Bool(required));
    if let Some(default) = spec.get("default") {
        entry.insert("default".into(), default.clone());
    }
    if let Some(description) = spec.get("description").and_then(|v| v.as_str()) {
        entry.insert("description".into(), Value::String(description.to_string()));
    }
    Value::Object(entry)
}

/// Compact a JSON Schema type into the catalog's flat type string. Arrays
/// turn into `T[]` (matching the existing catalog convention); enums collapse
/// to their base type and the values stay visible via the description.
fn parameter_type(spec: &Value) -> String {
    let base = spec.get("type").and_then(|v| v.as_str()).unwrap_or("any");
    if base == "array" {
        if let Some(items) = spec.get("items") {
            let item_type = items
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("object");
            return format!("{item_type}[]");
        }
        return "array".to_string();
    }
    base.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the checked-in `agents/tool-catalog.json` is byte-identical
    /// to what the generator would emit. If this test fails, regenerate with:
    /// `cargo run -p vulcan --bin gen_tool_catalog > agents/tool-catalog.json`.
    #[test]
    fn checked_in_catalog_matches_generator() {
        let generated = serde_json::to_string_pretty(&generate())
            .expect("catalog serialization should not fail");
        let checked_in = include_str!("../../../agents/tool-catalog.json").trim_end_matches('\n');
        let generated = generated.as_str().trim_end_matches('\n');
        assert_eq!(
            generated, checked_in,
            "agents/tool-catalog.json is stale. Regenerate with: \
             cargo run -p vulcan --bin gen_tool_catalog > agents/tool-catalog.json"
        );
    }

    #[test]
    fn parameters_extract_required_flag_correctly() {
        let schema = json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" },
                "depth":  { "type": "integer", "default": 10 }
            },
            "required": ["symbol"]
        });
        let params = parameters_from_schema(&schema);
        let symbol = params.iter().find(|p| p["name"] == "symbol").unwrap();
        let depth = params.iter().find(|p| p["name"] == "depth").unwrap();
        assert_eq!(symbol["required"], Value::Bool(true));
        assert_eq!(depth["required"], Value::Bool(false));
        assert_eq!(depth["default"], Value::Number(10.into()));
    }

    #[test]
    fn array_types_render_as_suffixed_strings() {
        let spec = json!({ "type": "array", "items": { "type": "string" } });
        assert_eq!(parameter_type(&spec), "string[]");
        let spec = json!({ "type": "array", "items": { "type": "object" } });
        assert_eq!(parameter_type(&spec), "object[]");
    }
}
