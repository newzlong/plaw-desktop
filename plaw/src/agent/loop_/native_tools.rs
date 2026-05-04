//! Helpers that convert plaw's internal tool representations to/from
//! the on-the-wire native tool-call shapes that providers (OpenAI,
//! Anthropic, OpenRouter, Kimi-Anthropic-compat) expect.
//!
//! When we record an assistant turn that issued tool calls, we encode
//! it as JSON so the next provider call can reconstruct the proper
//! assistant message with structured `tool_calls`. The OpenRouter
//! provider's `convert_messages` parses this JSON back. See
//! [`build_native_assistant_history`] (provider-issued `ToolCall`s)
//! and [`build_native_assistant_history_from_parsed_calls`]
//! (prompt-mode-parsed `ParsedToolCall`s when the provider didn't
//! return native tool_calls and we extracted them from text).

use crate::providers::ToolCall;

use super::parsing::ParsedToolCall;

/// Build assistant history entry in JSON format for native tool-call APIs.
/// `convert_messages` in the OpenRouter provider parses this JSON to
/// reconstruct the proper `NativeMessage` with structured `tool_calls`.
///
/// Empty `text` is encoded as JSON `null` (not the empty string) because
/// some providers (Kimi via Anthropic-compat) reject assistant turns
/// whose content is `""`.
pub(super) fn build_native_assistant_history(
    text: &str,
    tool_calls: &[ToolCall],
    reasoning_content: Option<&str>,
) -> String {
    let calls_json: Vec<serde_json::Value> = tool_calls
        .iter()
        .map(|tc| {
            serde_json::json!({
                "id": tc.id,
                "name": tc.name,
                "arguments": tc.arguments,
            })
        })
        .collect();

    let content = if text.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(text.trim().to_string())
    };

    let mut obj = serde_json::json!({
        "content": content,
        "tool_calls": calls_json,
    });

    if let Some(rc) = reasoning_content {
        obj.as_object_mut().unwrap().insert(
            "reasoning_content".to_string(),
            serde_json::Value::String(rc.to_string()),
        );
    }

    obj.to_string()
}

/// Build assistant history from prompt-mode-parsed tool calls.
///
/// Returns `None` when any parsed call is missing its `tool_call_id` —
/// without an id we can't construct a valid native-shape `tool_calls`
/// entry, and the caller falls back to the plain text path.
pub(super) fn build_native_assistant_history_from_parsed_calls(
    text: &str,
    tool_calls: &[ParsedToolCall],
    reasoning_content: Option<&str>,
) -> Option<String> {
    let calls_json = tool_calls
        .iter()
        .map(|tc| {
            Some(serde_json::json!({
                "id": tc.tool_call_id.clone()?,
                "name": tc.name,
                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string()),
            }))
        })
        .collect::<Option<Vec<_>>>()?;

    let content = if text.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(text.trim().to_string())
    };

    let mut obj = serde_json::json!({
        "content": content,
        "tool_calls": calls_json,
    });

    if let Some(rc) = reasoning_content {
        obj.as_object_mut().unwrap().insert(
            "reasoning_content".to_string(),
            serde_json::Value::String(rc.to_string()),
        );
    }

    Some(obj.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_encoded_as_json_null_not_empty_string() {
        // Some providers (Kimi via Anthropic-compat) reject assistant
        // turns whose content is the empty string. The encoder MUST
        // emit JSON `null` for blank text.
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: r#"{"command":"ls"}"#.into(),
        }];
        let out = build_native_assistant_history("", &calls, None);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["content"].is_null(), "empty text should encode as JSON null, got {parsed:?}");
    }

    #[test]
    fn whitespace_only_text_also_encodes_as_null() {
        // The "trim().is_empty()" branch must collapse whitespace-only
        // strings (e.g. `"\n  "` from a streaming chunk boundary) to
        // null too — otherwise we re-introduce the same Kimi reject.
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
        }];
        let out = build_native_assistant_history("   \n\t", &calls, None);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["content"].is_null());
    }

    #[test]
    fn reasoning_content_when_some_is_attached_at_top_level() {
        let calls = vec![];
        let out = build_native_assistant_history("ok", &calls, Some("scratch thoughts"));
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["reasoning_content"], "scratch thoughts");
    }

    #[test]
    fn reasoning_content_when_none_is_omitted_not_set_to_null() {
        // Distinguishing "no reasoning" from "empty reasoning" matters
        // because some providers special-case the presence of the field.
        let out = build_native_assistant_history("ok", &[], None);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.get("reasoning_content").is_none());
    }

    #[test]
    fn parsed_calls_path_returns_none_when_id_missing() {
        // Prompt-mode parsing sometimes can't recover a tool_call_id
        // (e.g. raw `<tool_call>` XML without the id field). Without
        // an id we can't emit a valid native shape; the caller is
        // expected to fall back to the plain-text history path.
        let parsed = vec![ParsedToolCall {
            name: "shell".into(),
            arguments: serde_json::json!({"command": "ls"}),
            tool_call_id: None,
        }];
        let out = build_native_assistant_history_from_parsed_calls("ok", &parsed, None);
        assert!(out.is_none(), "missing id must short-circuit to None");
    }

    #[test]
    fn parsed_calls_path_emits_arguments_as_json_string() {
        // Native tool_calls expect `arguments` as a *string* (JSON
        // serialised), not a parsed JSON object. Pinning this here
        // catches a subtle regression where the arguments get embedded
        // as an object and the downstream provider then re-parses
        // them double-quoted.
        let parsed = vec![ParsedToolCall {
            name: "shell".into(),
            arguments: serde_json::json!({"command": "ls"}),
            tool_call_id: Some("call_42".into()),
        }];
        let out = build_native_assistant_history_from_parsed_calls("ok", &parsed, None).unwrap();
        let parsed_back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed_back["tool_calls"][0]["arguments"].is_string());
    }

}
