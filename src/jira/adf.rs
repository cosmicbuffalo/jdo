//! Atlassian Document Format (ADF) support.
//!
//! P1 implements read-only flattening: `to_text` walks the ADF tree and pulls
//! out text with block-level newlines — enough to display description /
//! acceptance criteria / comments. Rich rendering (`to_lines`) and authoring
//! (`text_to_adf`) land in P2/P3.

use serde_json::Value;

/// Flatten an ADF node (or `None`/`Null`) into plain text.
pub fn to_text(node: Option<&Value>) -> String {
    let mut out = String::new();
    if let Some(v) = node {
        walk(v, &mut out);
    }
    out.trim().to_string()
}

fn walk(v: &Value, out: &mut String) {
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
                _ => {}
            }
            if let Some(content) = map.get("content") {
                walk(content, out);
            }
            // Newline after block-level nodes so paragraphs/list items separate.
            if matches!(
                node_type,
                "paragraph" | "heading" | "listItem" | "blockquote" | "codeBlock"
            ) {
                out.push('\n');
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, out);
            }
        }
        _ => {}
    }
}
