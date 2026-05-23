//! Runtime JSON-Schema validation for tool arguments.
//!
//! Every [`crate::tools::Tool`] advertises a `parameters_schema()` to the
//! LLM, but historically each tool then re-parsed `serde_json::Value` ad-hoc
//! and returned an opaque "Missing 'x' parameter" error. This module is the
//! single chokepoint that validates the LLM's actual args against the
//! advertised schema BEFORE `execute()` runs, returning a structured error
//! that the LLM can act on without a guess-and-retry round-trip.
//!
//! Wired in via the [`crate::tools::Tool::execute_validated`] default trait
//! method; existing tool implementations need zero changes.

use serde_json::{json, Value};

/// A single validation failure, structured so the LLM can identify the
/// exact path and rule that failed and re-try with corrected args.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidationFailure {
    /// JSON Pointer to the failing instance (RFC 6901). Empty string at root.
    pub path: String,
    /// Short rule name from JSON Schema vocabulary
    /// (`required`, `type`, `maximum`, `minimum`, `pattern`, etc.).
    pub rule: String,
    /// Human-readable message from the validator.
    pub message: String,
}

/// Validate `args` against `schema`. Returns `Ok(())` on success, or a
/// non-empty `Vec<ValidationFailure>` listing every failure found.
pub fn validate_against_schema(
    args: &Value,
    schema: &Value,
) -> Result<(), Vec<ValidationFailure>> {
    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(err) => {
            // Misconfigured tool: schema itself doesn't compile. Surface as
            // a single failure rather than panicking — the tool author
            // should fix it but the agent loop shouldn't die.
            return Err(vec![ValidationFailure {
                path: String::new(),
                rule: "schema_compile_error".into(),
                message: format!("tool parameters_schema() is invalid: {err}"),
            }]);
        }
    };

    let failures: Vec<ValidationFailure> = validator
        .iter_errors(args)
        .map(|err| ValidationFailure {
            path: err.instance_path.to_string(),
            rule: kind_short_name(&err.kind),
            message: err.to_string(),
        })
        .collect();

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

/// Extract a short, stable rule name from the validator's Debug-only error
/// kind enum. Produces lowercase tokens matching JSON Schema vocabulary
/// (`required`, `type`, `maximum`, `pattern`, …) so the LLM can pattern-
/// match and retry. Falls back to `"unknown"` if the variant shape
/// changes upstream.
fn kind_short_name(kind: &jsonschema::error::ValidationErrorKind) -> String {
    let dbg = format!("{kind:?}");
    dbg.split(['{', ' ', '(', ':'])
        .next()
        .unwrap_or("unknown")
        .to_ascii_lowercase()
}

/// Render a list of failures as the JSON payload that goes into
/// `ToolResult.error`. Matches the structured-error contract the LLM is
/// instructed to retry on.
pub fn render_validation_error(failures: &[ValidationFailure]) -> String {
    let payload = json!({ "validation_error": failures });
    payload.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "minLength": 1 },
                "timeout": { "type": "integer", "minimum": 1, "maximum": 600 }
            },
            "required": ["command"]
        })
    }

    #[test]
    fn valid_args_pass() {
        let args = json!({ "command": "ls", "timeout": 30 });
        assert!(validate_against_schema(&args, &shell_schema()).is_ok());
    }

    #[test]
    fn missing_required_field_fails() {
        let args = json!({ "timeout": 30 });
        let failures = validate_against_schema(&args, &shell_schema()).unwrap_err();
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0].rule.contains("required") || failures[0].message.contains("command"),
            "expected required-rule failure for 'command', got {failures:?}"
        );
    }

    #[test]
    fn wrong_type_fails() {
        let args = json!({ "command": 123 });
        let failures = validate_against_schema(&args, &shell_schema()).unwrap_err();
        assert!(!failures.is_empty());
        assert!(
            failures.iter().any(|f| f.rule.contains("type")),
            "expected type rule, got {failures:?}"
        );
    }

    #[test]
    fn maximum_constraint_fails() {
        let args = json!({ "command": "ls", "timeout": 9999 });
        let failures = validate_against_schema(&args, &shell_schema()).unwrap_err();
        assert!(
            failures.iter().any(|f| f.rule.contains("maximum")),
            "expected maximum-rule failure, got {failures:?}"
        );
        assert!(
            failures.iter().any(|f| f.path.contains("timeout")),
            "expected path to mention timeout, got {failures:?}"
        );
    }

    #[test]
    fn multiple_failures_all_reported() {
        // Both wrong type AND missing required
        let args = json!({ "timeout": "not a number" });
        let failures = validate_against_schema(&args, &shell_schema()).unwrap_err();
        assert!(failures.len() >= 2, "expected ≥2 failures, got {failures:?}");
    }

    #[test]
    fn render_produces_machine_readable_json() {
        let failures = vec![ValidationFailure {
            path: "/timeout".into(),
            rule: "maximum".into(),
            message: "9999 is greater than maximum 600".into(),
        }];
        let rendered = render_validation_error(&failures);
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert!(parsed.get("validation_error").is_some());
        assert_eq!(parsed["validation_error"][0]["rule"], "maximum");
        assert_eq!(parsed["validation_error"][0]["path"], "/timeout");
    }

    #[test]
    fn empty_object_against_empty_required_passes() {
        let schema = json!({ "type": "object" });
        let args = json!({});
        assert!(validate_against_schema(&args, &schema).is_ok());
    }

    #[test]
    fn malformed_schema_returns_compile_error_failure() {
        // Invalid: "required" must be array of strings, not a string
        let bad_schema = json!({
            "type": "object",
            "required": "not-an-array"
        });
        let args = json!({});
        let failures = validate_against_schema(&args, &bad_schema).unwrap_err();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].rule, "schema_compile_error");
    }
}
