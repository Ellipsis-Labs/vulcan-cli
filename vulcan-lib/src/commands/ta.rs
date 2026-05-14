//! Technical analysis command execution.

use std::collections::BTreeMap;
use std::str::FromStr;

use crate::cli::ta::TaCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::indicators::{self, IndicatorKind, IndicatorRequest, TriggerSpec};
use crate::output::render_success;

pub async fn execute(ctx: &AppContext, cmd: TaCommand) -> Result<(), VulcanError> {
    match cmd {
        TaCommand::Compute {
            symbol,
            indicator,
            timeframe,
            period,
            limit,
            params,
        } => {
            let kind = IndicatorKind::from_str(&indicator)?;
            let parsed_params = parse_params(params.as_deref())?;
            let request = IndicatorRequest {
                kind,
                period,
                params: parsed_params,
            };
            let series = indicators::compute(ctx, &symbol, &timeframe, &request, limit).await?;
            render_success(
                ctx.output_format,
                &series,
                serde_json::json!({
                    "command": "ta compute",
                    "symbol": symbol,
                    "indicator": indicator,
                    "timeframe": timeframe,
                }),
            );
            Ok(())
        }

        TaCommand::Signal { symbol, spec } => {
            let spec: TriggerSpec = serde_json::from_str(&spec).map_err(|e| {
                VulcanError::validation(
                    "INVALID_TRIGGER_SPEC",
                    format!("could not parse --spec JSON: {e}"),
                )
            })?;
            let outcome = indicators::evaluate_trigger(ctx, &symbol, &spec).await?;
            render_success(
                ctx.output_format,
                &outcome,
                serde_json::json!({
                    "command": "ta signal",
                    "symbol": symbol,
                }),
            );
            Ok(())
        }

        TaCommand::Report { symbol, timeframe } => {
            let report = indicators::report(ctx, &symbol, &timeframe).await?;
            render_success(
                ctx.output_format,
                &report,
                serde_json::json!({
                    "command": "ta report",
                    "symbol": symbol,
                    "timeframe": timeframe,
                }),
            );
            Ok(())
        }
    }
}

fn parse_params(raw: Option<&str>) -> Result<BTreeMap<String, f64>, VulcanError> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        VulcanError::validation(
            "INVALID_INDICATOR_PARAMS",
            format!("--params must be a JSON object of name→number: {e}"),
        )
    })?;
    let obj = value.as_object().ok_or_else(|| {
        VulcanError::validation(
            "INVALID_INDICATOR_PARAMS",
            "--params must be a JSON object, e.g. '{\"fast\":12,\"slow\":26}'",
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        let n = v.as_f64().ok_or_else(|| {
            VulcanError::validation(
                "INVALID_INDICATOR_PARAMS",
                format!("--params value for '{k}' must be a number"),
            )
        })?;
        out.insert(k.clone(), n);
    }
    Ok(out)
}
