//! Shared JSON extraction helpers used across the Fantrax and MLB modules.

use serde_json::Value;

/// Try multiple field names to extract an `f64` from a JSON object.
///
/// Handles numeric values, integer values, and string-encoded numbers.
/// Returns `0.0` if no key matches.
pub fn json_f64(obj: &Value, keys: &[&str]) -> f64 {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            if let Some(n) = v.as_f64() {
                return n;
            }
            if let Some(n) = v.as_i64() {
                return n as f64;
            }
            if let Some(s) = v.as_str()
                && let Ok(n) = s.parse::<f64>()
            {
                return n;
            }
        }
    }
    0.0
}

/// Try multiple field names to extract a `u32` from a JSON object.
///
/// Handles integer values, float values (truncated), and string-encoded numbers.
/// Returns `None` if no key matches.
pub fn json_u32(obj: &Value, keys: &[&str]) -> Option<u32> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            if let Some(n) = v.as_u64() {
                return Some(n as u32);
            }
            if let Some(n) = v.as_f64() {
                return Some(n as u32);
            }
            if let Some(s) = v.as_str()
                && let Ok(n) = s.parse::<u32>()
            {
                return Some(n);
            }
        }
    }
    None
}
