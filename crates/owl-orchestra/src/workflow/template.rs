//! Minimal `{{...}}` template renderer for workflow step prompts.
//!
//! Supported forms (Design Spec §4):
//! - `{{user_input}}`              — the workflow's initial input string.
//! - `{{<step_id>.output}}`        — verbatim text output of a prior step.
//! - `{{<step_id>.json.<dotpath>}}` — if the step output parses as JSON,
//!   walk dotted path (`a.b[0].c`).  Unknown path → error.
//! - `{{env.<NAME>}}`              — read-only env var (allowlist
//!   `OWL_*` and `WORKSPACE`; everything else is rejected).
//!
//! Strict mode: an unknown variable, a malformed path, or a forbidden env
//! var raises an error rather than substituting an empty string.  Workflow
//! authors get loud feedback, not silent corruption.

use std::collections::HashMap;

use owl_protocol::orchestra::StepId;

/// Render a template against the supplied context.  See module docs for
/// supported variable forms.
pub fn render(
    template:  &str,
    user_input: &str,
    outputs:   &HashMap<StepId, String>,
) -> Result<String, RenderError> {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;

    while let Some(open_at) = rest.find("{{") {
        // Copy literal text up to the `{{`.
        out.push_str(&rest[..open_at]);
        let after_open = &rest[open_at + 2..];
        let close_rel = after_open.find("}}")
            .ok_or_else(|| RenderError::Unclosed(open_at))?;
        let expr = after_open[..close_rel].trim();

        // Resolve the variable.
        let value = resolve(expr, user_input, outputs)?;
        out.push_str(&value);

        rest = &after_open[close_rel + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve(
    expr:       &str,
    user_input: &str,
    outputs:    &HashMap<StepId, String>,
) -> Result<String, RenderError> {
    // user_input — bare reserved name
    if expr == "user_input" {
        return Ok(user_input.to_string());
    }

    // env.<NAME>
    if let Some(name) = expr.strip_prefix("env.") {
        if !is_env_allowed(name) {
            return Err(RenderError::EnvForbidden(name.to_string()));
        }
        return std::env::var(name)
            .map_err(|_| RenderError::EnvMissing(name.to_string()));
    }

    // <step_id>.output  or  <step_id>.json.<path>
    if let Some(dot) = expr.find('.') {
        let (step, suffix) = (expr[..dot].trim(), expr[dot + 1..].trim());
        let step_id = StepId::new_reserved(step)
            .map_err(|e| RenderError::InvalidStep { step: step.to_string(), reason: e.to_string() })?;
        let output = outputs.get(&step_id)
            .ok_or_else(|| RenderError::UnknownStep(step.to_string()))?;

        if suffix == "output" {
            return Ok(output.clone());
        }
        if let Some(json_path) = suffix.strip_prefix("json.") {
            return resolve_json_path(output, json_path);
        }
        return Err(RenderError::UnknownField {
            step: step.to_string(),
            field: suffix.to_string(),
        });
    }

    Err(RenderError::UnknownVariable(expr.to_string()))
}

/// Allow only `OWL_*` and `WORKSPACE` env vars to be inlined into prompts —
/// avoids accidentally leaking the user's `PATH`, `HOME`, or arbitrary
/// secret env vars into LLM context.
fn is_env_allowed(name: &str) -> bool {
    name == "WORKSPACE" || name.starts_with("OWL_")
}

/// Walk a dotted path through parsed JSON, supporting numeric subscripts via
/// dot-prefix (`a.0.b`).  Treats the leaf as a string; non-string leaves are
/// JSON-encoded (`{"a":1}` → `{\"a\":1}`).
fn resolve_json_path(raw: &str, path: &str) -> Result<String, RenderError> {
    let mut value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| RenderError::JsonParse(e.to_string()))?;

    for segment in path.split('.') {
        if segment.is_empty() { continue; }
        let next = match &mut value {
            serde_json::Value::Object(map) => map.remove(segment),
            serde_json::Value::Array(arr)  => segment.parse::<usize>().ok()
                .and_then(|i| if i < arr.len() { Some(arr.swap_remove(i)) } else { None }),
            _ => None,
        };
        value = next.ok_or_else(|| RenderError::JsonPath {
            path: path.to_string(),
            failed_at: segment.to_string(),
        })?;
    }

    Ok(match value {
        serde_json::Value::String(s) => s,
        other                        => other.to_string(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("unclosed `{{{{` at byte offset {0}")]
    Unclosed(usize),
    #[error("unknown variable `{0}`")]
    UnknownVariable(String),
    #[error("unknown step `{0}`")]
    UnknownStep(String),
    #[error("step `{step}` has no field `{field}` (expected `output` or `json.<path>`)")]
    UnknownField { step: String, field: String },
    #[error("invalid step name `{step}`: {reason}")]
    InvalidStep { step: String, reason: String },
    #[error("env var `{0}` not in allowlist (only OWL_* and WORKSPACE)")]
    EnvForbidden(String),
    #[error("env var `{0}` is not set")]
    EnvMissing(String),
    #[error("output is not valid JSON: {0}")]
    JsonParse(String),
    #[error("JSON path `{path}` failed at segment `{failed_at}`")]
    JsonPath { path: String, failed_at: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs(pairs: &[(&str, &str)]) -> HashMap<StepId, String> {
        pairs.iter()
            .map(|(k, v)| (StepId::new_reserved(*k).unwrap(), v.to_string()))
            .collect()
    }

    #[test]
    fn substitutes_user_input() {
        let r = render("Hello {{user_input}}!", "world", &outputs(&[])).unwrap();
        assert_eq!(r, "Hello world!");
    }

    #[test]
    fn substitutes_step_output() {
        let r = render("Plan: {{plan.output}}", "ignored", &outputs(&[("plan", "DO X")])).unwrap();
        assert_eq!(r, "Plan: DO X");
    }

    #[test]
    fn unknown_step_errors() {
        let err = render("{{ghost.output}}", "", &outputs(&[])).unwrap_err();
        assert!(matches!(err, RenderError::UnknownStep(_)));
    }

    #[test]
    fn unknown_variable_errors() {
        let err = render("{{nope}}", "", &outputs(&[])).unwrap_err();
        assert!(matches!(err, RenderError::UnknownVariable(_)));
    }

    #[test]
    fn json_path_resolves() {
        let json = r#"{"files":["a.rs","b.rs"]}"#;
        let r = render("first: {{plan.json.files.0}}", "", &outputs(&[("plan", json)])).unwrap();
        assert_eq!(r, "first: a.rs");
    }

    #[test]
    fn json_path_missing_errors() {
        let json = r#"{"x":1}"#;
        let err = render("{{plan.json.missing}}", "", &outputs(&[("plan", json)])).unwrap_err();
        assert!(matches!(err, RenderError::JsonPath { .. }));
    }

    #[test]
    fn env_allowlist_blocks_path() {
        std::env::set_var("PATH", "/bin");
        let err = render("{{env.PATH}}", "", &outputs(&[])).unwrap_err();
        assert!(matches!(err, RenderError::EnvForbidden(_)));
    }

    #[test]
    fn env_owl_allowed() {
        std::env::set_var("OWL_TEST_VAR", "ok");
        let r = render("{{env.OWL_TEST_VAR}}", "", &outputs(&[])).unwrap();
        assert_eq!(r, "ok");
    }

    #[test]
    fn unclosed_braces_error() {
        let err = render("hello {{user_input", "", &outputs(&[])).unwrap_err();
        assert!(matches!(err, RenderError::Unclosed(_)));
    }

    #[test]
    fn literal_text_passes_through() {
        let r = render("just text, no vars", "x", &outputs(&[])).unwrap();
        assert_eq!(r, "just text, no vars");
    }
}
