//! JSON envelope types for structured output.

use crate::error::VulcanError;
use serde::Serialize;

/// Successful response envelope.
#[derive(Debug, Serialize)]
pub struct SuccessEnvelope<T: Serialize> {
    pub ok: bool,
    pub data: T,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

impl<T: Serialize> SuccessEnvelope<T> {
    pub fn new(data: T, meta: serde_json::Value) -> Self {
        Self {
            ok: true,
            data,
            meta,
        }
    }
}

/// Error detail inside the error envelope.
#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub category: String,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

/// Error response envelope.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub ok: bool,
    pub error: ErrorDetail,
}

impl ErrorEnvelope {
    pub fn from_error(err: &VulcanError) -> Self {
        Self {
            ok: false,
            error: ErrorDetail {
                category: err.category.to_string(),
                code: err.code.clone(),
                message: err.message.clone(),
                retryable: err.category.is_retryable(),
                hint: err.recovery_hint().to_string(),
            },
        }
    }
}
