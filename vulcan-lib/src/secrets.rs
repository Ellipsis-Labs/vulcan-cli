//! Redacting wrappers for in-memory secrets.
//!
//! Anything that holds an unredacted password, decrypted session token, or
//! API secret should go through [`SecretString`]. The wrapper's `Debug`,
//! `Display`, and `Serialize` impls all emit `[REDACTED]`, so:
//!
//! - `tracing::debug!("{:?}", creds)` cannot leak the secret.
//! - Panic backtraces that print struct fields cannot leak it.
//! - JSON envelopes that serialize a struct holding it cannot leak it.
//! - `Drop` zeroizes the underlying bytes.
//!
//! Use `.expose()` when the inner value is genuinely needed (decrypt call,
//! HTTP auth header). Callers must not log that return.

use serde::{Serialize, Serializer};
use std::fmt;
use zeroize::Zeroize;

/// A `String` that never reveals itself in `Debug` / `Display` / `Serialize`
/// output, and is zeroized on drop.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Read-only access to the inner secret. **Callers must not log the result.**
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl Zeroize for SecretString {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Display helper for **public** identifiers (API key prefix, wallet pubkey
/// tail) where some surface is useful for disambiguation across multiple
/// stored items. Returns `***last4` for inputs ≥ 4 chars; `***` otherwise.
///
/// Do NOT use this for secrets — secrets should be wrapped in [`SecretString`]
/// and rendered as `[REDACTED]`.
pub fn mask_string(s: &str) -> String {
    if s.len() >= 4 {
        format!("***{}", &s[s.len() - 4..])
    } else {
        "***".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_leaks() {
        let s = SecretString::new("super-secret-password".into());
        let dbg = format!("{:?}", s);
        assert!(
            !dbg.contains("super-secret"),
            "Debug leaked secret material: {dbg}"
        );
        assert!(dbg.contains("REDACTED"));
    }

    #[test]
    fn display_never_leaks() {
        let s = SecretString::new("super-secret-password".into());
        let display = format!("{}", s);
        assert!(
            !display.contains("super-secret"),
            "Display leaked secret material: {display}"
        );
        assert_eq!(display, "[REDACTED]");
    }

    #[test]
    fn serialize_never_leaks() {
        let s = SecretString::new("super-secret-password".into());
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            !json.contains("super-secret"),
            "Serialize leaked secret material: {json}"
        );
        assert_eq!(json, "\"[REDACTED]\"");
    }

    #[test]
    fn expose_returns_inner() {
        let s = SecretString::new("pw".into());
        assert_eq!(s.expose(), "pw");
        assert_eq!(s.len(), 2);
        assert!(!s.is_empty());
    }

    /// Debug a parent struct that contains a SecretString — the secret must
    /// not appear in the formatted output even when the parent uses
    /// `#[derive(Debug)]`.
    #[test]
    fn parent_struct_debug_does_not_leak() {
        #[derive(Debug)]
        struct Creds {
            #[allow(dead_code)]
            name: String,
            #[allow(dead_code)]
            password: SecretString,
        }
        let c = Creds {
            name: "alice".into(),
            password: SecretString::new("super-secret".into()),
        };
        let dbg = format!("{:?}", c);
        assert!(!dbg.contains("super-secret"), "leaked via parent: {dbg}");
        assert!(
            dbg.contains("alice"),
            "non-secret field should still render"
        );
    }

    #[test]
    fn mask_string_handles_short_and_long() {
        assert_eq!(mask_string("abcdef"), "***cdef");
        assert_eq!(mask_string("abc"), "***");
        assert_eq!(mask_string(""), "***");
        assert_eq!(mask_string("1234"), "***1234");
    }
}
