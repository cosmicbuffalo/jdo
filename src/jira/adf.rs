//! Atlassian Document Format (ADF) support.
//!
//! `description`, rich-text custom fields (e.g. Acceptance Criteria) and
//! comment bodies are ADF trees. [`to_text`] flattens one for display, keeping
//! paragraph breaks and list bullets; [`text_to_adf`] turns edited plain text
//! back into a minimal ADF document (paragraphs + hard breaks). The round trip
//! is lossy for rich formatting — acceptable for the MVP.

use serde_json::{Value, json};

/// Flatten an ADF node (or `None`/`Null`) into plain text.
pub fn to_text(node: Option<&Value>) -> String {
    // The Agile API (v1) returns rich-text fields as plain strings, not ADF.
    if let Some(Value::String(text)) = node {
        return text.trim().to_string();
    }
    let mut out = String::new();
    if let Some(v) = node {
        walk(v, &mut out, &mut Vec::new());
    }
    // Collapse runs of 3+ newlines left by nested blocks.
    let mut text = out.trim().to_string();
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text
}

/// List context: `None` for bullet lists, `Some(n)` for the next ordered number.
type ListStack = Vec<Option<u64>>;

fn walk(v: &Value, out: &mut String, lists: &mut ListStack) {
    match v {
        Value::Object(map) => {
            let node_type = map.get("type").and_then(Value::as_str).unwrap_or("");
            match node_type {
                "text" => {
                    if let Some(s) = map.get("text").and_then(Value::as_str) {
                        out.push_str(s);
                    }
                }
                "hardBreak" => out.push('\n'),
                "mention" | "emoji" | "status" => {
                    let attrs = map.get("attrs");
                    let text = attrs
                        .and_then(|a| a.get("text").or_else(|| a.get("shortName")))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    out.push_str(text);
                }
                "inlineCard" | "blockCard" => {
                    if let Some(url) = map
                        .get("attrs")
                        .and_then(|a| a.get("url"))
                        .and_then(Value::as_str)
                    {
                        out.push_str(url);
                    }
                }
                "rule" => out.push_str("────────\n"),
                "bulletList" => lists.push(None),
                "orderedList" => {
                    let start = map
                        .get("attrs")
                        .and_then(|a| a.get("order"))
                        .and_then(Value::as_u64)
                        .unwrap_or(1);
                    lists.push(Some(start));
                }
                "listItem" => {
                    let depth = lists.len().saturating_sub(1);
                    out.push_str(&"  ".repeat(depth));
                    match lists.last_mut() {
                        Some(Some(n)) => {
                            out.push_str(&format!("{n}. "));
                            *n += 1;
                        }
                        _ => out.push_str("• "),
                    }
                }
                _ => {}
            }
            if let Some(content) = map.get("content") {
                walk(content, out, lists);
            }
            match node_type {
                "bulletList" | "orderedList" => {
                    lists.pop();
                    if lists.is_empty() {
                        out.push('\n');
                    }
                }
                // A paragraph inside a list item ends the item's line only.
                "paragraph" if !lists.is_empty() => out.push('\n'),
                "paragraph" | "heading" | "blockquote" | "codeBlock" | "panel" => {
                    out.push_str("\n\n")
                }
                _ => {}
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, out, lists);
            }
        }
        _ => {}
    }
}

/// Convert plain text into a minimal ADF document: blank lines separate
/// paragraphs; single newlines become hard breaks.
pub fn text_to_adf(text: &str) -> Value {
    let normalized = text.replace("\r\n", "\n");
    let paragraphs: Vec<Value> = normalized
        .split("\n\n")
        .map(|p| p.trim_matches('\n'))
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let mut content = Vec::new();
            for (i, line) in p.split('\n').enumerate() {
                if i > 0 {
                    content.push(json!({"type": "hardBreak"}));
                }
                if !line.is_empty() {
                    content.push(json!({"type": "text", "text": line}));
                }
            }
            json!({"type": "paragraph", "content": content})
        })
        .collect();
    json!({"type": "doc", "version": 1, "content": paragraphs})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_paragraphs_and_lists() {
        let doc = json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Intro"}]},
            {"type": "bulletList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "one"}]}]},
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "two"}]}]}
            ]},
            {"type": "orderedList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "first"}]}]}
            ]}
        ]});
        assert_eq!(to_text(Some(&doc)), "Intro\n\n• one\n• two\n\n1. first");
    }

    #[test]
    fn plain_string_fields_pass_through() {
        assert_eq!(to_text(Some(&json!("  wiki text  "))), "wiki text");
    }

    #[test]
    fn null_and_missing_flatten_to_empty() {
        assert_eq!(to_text(None), "");
        assert_eq!(to_text(Some(&Value::Null)), "");
    }

    #[test]
    fn text_round_trips_through_adf() {
        let text = "Line one\nline two\n\nSecond paragraph";
        let doc = text_to_adf(text);
        assert_eq!(doc["content"].as_array().unwrap().len(), 2);
        assert_eq!(to_text(Some(&doc)), text);
    }

    #[test]
    fn empty_text_is_an_empty_doc() {
        assert_eq!(text_to_adf("  \n\n "), json!({"type": "doc", "version": 1, "content": []}));
    }
}
