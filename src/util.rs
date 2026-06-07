//! Shared JSON extraction helpers used across the Fantrax, MLB, and projections
//! modules.

use serde_json::Value;

/// Try multiple field names to extract a `String` from a JSON object.
///
/// Returns the first matching key's value as a `String`, or `None` if no key
/// matches or the value isn't a string.
pub fn json_str(obj: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = obj.get(*key).and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }
    None
}

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

/// Extract an `f64` from CSV fields at the given column index.
///
/// Returns `0.0` if the column index is `None`, out of bounds, or the value
/// can't be parsed.
pub fn csv_f64(fields: &[String], col: Option<usize>) -> f64 {
    col.and_then(|c| fields.get(c))
        .and_then(|s| s.trim().trim_matches('"').parse::<f64>().ok())
        .unwrap_or(0.0)
}
