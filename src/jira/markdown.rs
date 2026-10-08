//! ADF ↔ Markdown, for editing rich-text fields in a terminal.
//!
//! [`to_markdown`] turns an ADF document into Markdown the user can edit;
//! [`to_adf`] turns it back. Everything Markdown expresses round-trips:
//! paragraphs, headings, bullet/numbered/nested lists, code blocks, quotes,
//! rules, and bold/italic/code/strike/link marks. Anything else — tables,
//! panels, mentions, media, status lozenges — becomes a placeholder token
//! like `⟦3:table⟧`; left in place, the token is swapped back for the
//! original node on save, so those survive untouched. The only loss is marks
//! Markdown can't say (underline, text color, …), reported in `dropped`.
//!
//! Dialect notes: a single newline inside a paragraph is a hard break (as it
//! is in Jira's editor), `*x*` and `_x_` are italic, and `\` escapes markup.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

const TOKEN_OPEN: char = '⟦';
const TOKEN_CLOSE: char = '⟧';

/// An ADF document as editable Markdown.
#[derive(Debug, Clone, Default)]
pub struct Markdown {
    pub text: String,
    /// Original nodes behind the `⟦n:…⟧` placeholders, by `n`.
    pub tokens: Vec<Value>,
    /// Marks that can't be expressed and will be lost if the text is saved.
    pub dropped: Vec<String>,
}

impl Markdown {
    /// Readable names of the placeholder kinds, e.g. `["mention", "table"]`.
    pub fn placeholder_kinds(&self) -> Vec<String> {
        let kinds: BTreeSet<String> = self
            .tokens
            .iter()
            .filter_map(|t| t.get("type").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        kinds.into_iter().collect()
    }
}

// ----- ADF → Markdown -------------------------------------------------------

pub fn to_markdown(doc: Option<&Value>) -> Markdown {
    let mut writer = Writer::default();
    let text = match doc {
        // The Agile API hands back plain strings; nothing to convert.
        Some(Value::String(s)) => escape_lines(s.trim()),
        Some(doc) => writer.blocks(children(doc), 0).join("\n\n"),
        None => String::new(),
    };
    Markdown { text, tokens: writer.tokens, dropped: writer.dropped.into_iter().collect() }
}

#[derive(Default)]
struct Writer {
    tokens: Vec<Value>,
    dropped: BTreeSet<String>,
}

fn children(node: &Value) -> &[Value] {
    node.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn node_type(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or("")
}

fn attr<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    node.get("attrs").and_then(|a| a.get(key))
}

impl Writer {
    fn token(&mut self, node: &Value, label: &str) -> String {
        self.tokens.push(node.clone());
        format!("{TOKEN_OPEN}{}:{label}{TOKEN_CLOSE}", self.tokens.len() - 1)
    }

    fn blocks(&mut self, nodes: &[Value], depth: usize) -> Vec<String> {
        nodes.iter().map(|n| self.block(n, depth)).collect()
    }

    fn block(&mut self, node: &Value, depth: usize) -> String {
        match node_type(node) {
            "paragraph" => keep_blank_lines(&escape_block_start(&self.inline(children(node)))),
            "heading" => {
                let level = attr(node, "level").and_then(Value::as_u64).unwrap_or(1).clamp(1, 6) as usize;
                // Headings are one line in Markdown; a trailing `\` continues one.
                let text = self.inline(children(node)).split('\n').collect::<Vec<_>>().join("\\\n");
                format!("{} {}", "#".repeat(level), text)
            }
            "bulletList" | "orderedList" => self.list(node, depth),
            "codeBlock" => {
                let language = attr(node, "language").and_then(Value::as_str).unwrap_or("");
                let code: String = children(node).iter().filter_map(|t| t.get("text").and_then(Value::as_str)).collect();
                format!("```{language}\n{code}\n```")
            }
            "blockquote" => self
                .blocks(children(node), depth)
                .join("\n\n")
                .lines()
                .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
                .collect::<Vec<_>>()
                .join("\n"),
            "rule" => "---".into(),
            other => {
                let label = other.to_string();
                self.token(node, &label)
            }
        }
    }

    fn list(&mut self, node: &Value, depth: usize) -> String {
        let ordered = node_type(node) == "orderedList";
        let start = attr(node, "order").and_then(Value::as_u64).unwrap_or(1);
        let indent = "  ".repeat(depth);
        let mut lines = Vec::new();
        for (i, item) in children(node).iter().enumerate() {
            let marker = if ordered { format!("{}.", start + i as u64) } else { "-".into() };
            let hang = " ".repeat(marker.len() + 1);
            let mut first = true;
            for child in children(item) {
                match node_type(child) {
                    "bulletList" | "orderedList" => lines.push(self.list(child, depth + 1)),
                    "paragraph" => {
                        let text = keep_blank_lines(&escape_block_start(&self.inline(children(child))));
                        for (j, line) in text.split('\n').enumerate() {
                            if first && j == 0 {
                                lines.push(format!("{indent}{marker} {line}"));
                            } else {
                                lines.push(format!("{indent}{hang}{line}"));
                            }
                        }
                        first = false;
                    }
                    _ => {
                        let text = self.block(child, depth + 1);
                        let lead = if first { format!("{indent}{marker} ") } else { format!("{indent}{hang}") };
                        lines.push(format!("{lead}{text}"));
                        first = false;
                    }
                }
            }
            if first {
                lines.push(format!("{indent}{marker} "));
            }
        }
        lines.join("\n")
    }

    fn inline(&mut self, nodes: &[Value]) -> String {
        let mut out = String::new();
        for node in nodes {
            match node_type(node) {
                "text" => out.push_str(&self.marked(node)),
                "hardBreak" => out.push('\n'),
                kind => {
                    let label = match kind {
                        "mention" => attr(node, "text").and_then(Value::as_str).unwrap_or("@mention").to_string(),
                        "emoji" => attr(node, "text")
                            .or_else(|| attr(node, "shortName"))
                            .and_then(Value::as_str)
                            .unwrap_or("emoji")
                            .to_string(),
                        "inlineCard" => attr(node, "url").and_then(Value::as_str).unwrap_or("link").to_string(),
                        "status" => format!("status: {}", attr(node, "text").and_then(Value::as_str).unwrap_or("")),
                        other => other.to_string(),
                    };
                    let token = self.token(node, &label.replace(TOKEN_CLOSE, ""));
                    out.push_str(&token);
                }
            }
        }
        out
    }

    fn marked(&mut self, node: &Value) -> String {
        let raw = node.get("text").and_then(Value::as_str).unwrap_or("");
        let marks: Vec<&Value> = node.get("marks").and_then(Value::as_array).map(|m| m.iter().collect()).unwrap_or_default();
        let has = |name: &str| marks.iter().any(|m| node_type(m) == name);
        for mark in &marks {
            let name = node_type(mark);
            if !matches!(name, "strong" | "em" | "code" | "strike" | "link") {
                self.dropped.insert(name.to_string());
            }
        }
        // Innermost first, so the output reads **_text_** for [strong, em].
        let mut text = if has("code") { format!("`{}`", raw.replace('`', "'")) } else { escape_inline(raw) };
        if has("em") {
            text = format!("_{text}_");
        }
        if has("strong") {
            text = format!("**{text}**");
        }
        if has("strike") {
            text = format!("~~{text}~~");
        }
        if let Some(link) = marks.iter().find(|m| node_type(m) == "link") {
            let href = attr(link, "href").and_then(Value::as_str).unwrap_or("");
            text = format!("[{text}]({href})");
        }
        text
    }
}

/// A blank line would end the paragraph, so intentional empty lines (double
/// breaks, a break at either end, an empty paragraph) are written as `\`.
fn keep_blank_lines(text: &str) -> String {
    text.split('\n').map(|l| if l.is_empty() { "\\" } else { l }).collect::<Vec<_>>().join("\n")
}

/// Escape inline markup characters in plain text.
fn escape_inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        let at_word_edge = || {
            let before = i.checked_sub(1).map(|j| chars[j]);
            let after = chars.get(i + 1).copied();
            !before.is_some_and(char::is_alphanumeric) || !after.is_some_and(char::is_alphanumeric)
        };
        let escape = match c {
            '\\' | '*' | '`' | '[' | '~' | TOKEN_OPEN => true,
            // Underscores inside identifiers (CHECK_STANDARD) stay readable.
            '_' => at_word_edge(),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Plain strings (no ADF) still need escaping to survive the trip back.
fn escape_lines(text: &str) -> String {
    text.split('\n').map(|l| escape_block_start(&escape_inline(l))).collect::<Vec<_>>().join("\n")
}

/// Escape each line of a paragraph that would otherwise start a block.
fn escape_block_start(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            let starts_block = trimmed.starts_with('#')
                || trimmed.starts_with('>')
                || trimmed.starts_with("- ")
                || trimmed == "---"
                || ordered_marker(trimmed).is_some();
            if starts_block { format!("\\{line}") } else { line.to_string() }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ----- Markdown → ADF -------------------------------------------------------

/// Parse Markdown from the editor back into an ADF document. `tokens` are the
/// originals from [`to_markdown`]; placeholders that reference them are
/// restored verbatim.
pub fn to_adf(text: &str, tokens: &[Value]) -> Value {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    json!({"type": "doc", "version": 1, "content": parse_blocks(&lines, tokens)})
}

fn parse_blocks(lines: &[&str], tokens: &[Value]) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
        } else if let Some(language) = trimmed.strip_prefix("```") {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != "```" {
                code.push(lines[i]);
                i += 1;
            }
            i += 1; // closing fence
            let mut node = json!({"type": "codeBlock", "content": []});
            if !language.trim().is_empty() {
                node["attrs"] = json!({"language": language.trim()});
            }
            let code = code.join("\n");
            if !code.is_empty() {
                node["content"] = json!([{"type": "text", "text": code}]);
            }
            blocks.push(node);
        } else if let Some((level, first)) = heading(trimmed) {
            let mut parts = vec![first.to_string()];
            i += 1;
            while continues(parts.last().unwrap()) && i < lines.len() {
                let last = parts.last_mut().unwrap();
                last.pop();
                parts.push(lines[i].to_string());
                i += 1;
            }
            let mut content = Vec::new();
            for (n, part) in parts.iter().enumerate() {
                if n > 0 {
                    content.push(json!({"type": "hardBreak"}));
                }
                content.extend(parse_inline(part, tokens));
            }
            blocks.push(json!({"type": "heading", "attrs": {"level": level}, "content": content}));
        } else if trimmed == "---" {
            blocks.push(json!({"type": "rule"}));
            i += 1;
        } else if let Some(node) = block_token(trimmed, tokens) {
            blocks.push(node);
            i += 1;
        } else if trimmed.starts_with('>') {
            let mut quoted = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let rest = lines[i].trim_start()[1..].to_string();
                quoted.push(rest.strip_prefix(' ').map(str::to_string).unwrap_or(rest));
                i += 1;
            }
            let quoted: Vec<&str> = quoted.iter().map(String::as_str).collect();
            blocks.push(json!({"type": "blockquote", "content": parse_blocks(&quoted, tokens)}));
        } else if list_marker(line).is_some() {
            let start = i;
            i += 1;
            // A list runs until a blank line or an unindented non-item line.
            while i < lines.len()
                && !lines[i].trim().is_empty()
                && (list_marker(lines[i]).is_some() || lines[i].starts_with(' '))
            {
                i += 1;
            }
            blocks.push(parse_list(&lines[start..i], tokens));
        } else {
            let start = i;
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(lines[i], tokens) {
                i += 1;
            }
            blocks.push(paragraph(&lines[start..i], tokens));
        }
    }
    blocks
}

fn starts_block(line: &str, tokens: &[Value]) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("```")
        || heading(trimmed).is_some()
        || trimmed == "---"
        || trimmed.starts_with('>')
        || list_marker(line).is_some()
        || block_token(trimmed, tokens).is_some()
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    match &line[level..] {
        "" => Some((level, "")), // an empty heading
        rest => rest.strip_prefix(' ').map(|text| (level, text)),
    }
}

/// A heading line ending in an odd number of backslashes continues on the
/// next line (an even number is escaped backslashes).
fn continues(line: &str) -> bool {
    line.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

/// `"12. rest"` → `(12, "rest")`.
fn ordered_marker(line: &str) -> Option<(u64, &str)> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || !line[digits..].starts_with(". ") {
        return None;
    }
    Some((line[..digits].parse().ok()?, &line[digits + 2..]))
}

struct ListLine<'a> {
    indent: usize,
    /// `Some(n)` for a numbered item.
    number: Option<u64>,
    text: &'a str,
}

fn list_marker(line: &str) -> Option<ListLine<'_>> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    if let Some(text) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        return Some(ListLine { indent, number: None, text });
    }
    if rest == "-" || rest == "*" {
        return Some(ListLine { indent, number: None, text: "" });
    }
    ordered_marker(rest).map(|(n, text)| ListLine { indent, number: Some(n), text })
}

fn block_token(line: &str, tokens: &[Value]) -> Option<Value> {
    let (index, rest) = token_at(line)?;
    let node = tokens.get(index)?;
    // Only a token standing alone on its line, holding a block node.
    let inline_kinds = ["mention", "emoji", "inlineCard", "status", "date", "text", "hardBreak"];
    (rest.is_empty() && !inline_kinds.contains(&node_type(node))).then(|| node.clone())
}

/// `"⟦3:label⟧rest"` → `(3, "rest")`.
fn token_at(text: &str) -> Option<(usize, &str)> {
    let inner = text.strip_prefix(TOKEN_OPEN)?;
    let close = inner.find(TOKEN_CLOSE)?;
    let (body, rest) = (&inner[..close], &inner[close + TOKEN_CLOSE.len_utf8()..]);
    let index = body.split(':').next()?.parse().ok()?;
    Some((index, rest))
}

fn paragraph(lines: &[&str], tokens: &[Value]) -> Value {
    let mut content = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            content.push(json!({"type": "hardBreak"}));
        }
        if *line != "\\" {
            content.extend(parse_inline(line, tokens));
        }
    }
    json!({"type": "paragraph", "content": content})
}

/// Items with their nesting depth, then folded into nested ADF lists.
fn parse_list(lines: &[&str], tokens: &[Value]) -> Value {
    struct Item {
        depth: usize,
        number: Option<u64>,
        lines: Vec<String>,
    }
    let mut items: Vec<Item> = Vec::new();
    let mut indents: Vec<usize> = Vec::new();
    for line in lines {
        match list_marker(line) {
            Some(marker) => {
                while indents.last().is_some_and(|&i| i > marker.indent) {
                    indents.pop();
                }
                if indents.last() != Some(&marker.indent) {
                    indents.push(marker.indent);
                }
                items.push(Item { depth: indents.len() - 1, number: marker.number, lines: vec![marker.text.to_string()] });
            }
            // A continuation line belongs to the latest item.
            None => {
                if let Some(item) = items.last_mut() {
                    item.lines.push(line.trim().to_string());
                }
            }
        }
    }

    fn build(items: &[Item], index: &mut usize, depth: usize, tokens: &[Value]) -> Value {
        let first = &items[*index];
        let mut list = match first.number {
            Some(n) if n != 1 => json!({"type": "orderedList", "attrs": {"order": n}, "content": []}),
            Some(_) => json!({"type": "orderedList", "content": []}),
            None => json!({"type": "bulletList", "content": []}),
        };
        let mut content = Vec::new();
        while *index < items.len() && items[*index].depth >= depth {
            if items[*index].depth > depth {
                // Deeper than expected right away: nest under a blank item.
                let nested = build(items, index, depth + 1, tokens);
                content.push(json!({"type": "listItem", "content": [nested]}));
                continue;
            }
            let lines: Vec<&str> = items[*index].lines.iter().map(String::as_str).collect();
            let mut item = vec![paragraph(&lines, tokens)];
            *index += 1;
            if *index < items.len() && items[*index].depth > depth {
                item.push(build(items, index, depth + 1, tokens));
            }
            content.push(json!({"type": "listItem", "content": item}));
        }
        list["content"] = Value::Array(content);
        list
    }

    let mut index = 0;
    build(&items, &mut index, 0, tokens)
}

/// Inline Markdown → ADF text nodes with marks.
fn parse_inline(text: &str, tokens: &[Value]) -> Vec<Value> {
    let mut nodes = Vec::new();
    inline_into(text, &[], tokens, &mut nodes);
    // Merge neighbouring text nodes that carry the same marks.
    let mut merged: Vec<Value> = Vec::new();
    for node in nodes {
        if let (Some(last), "text") = (merged.last_mut(), node_type(&node)) {
            if node_type(last) == "text" && last.get("marks") == node.get("marks") {
                let joined = format!("{}{}", last["text"].as_str().unwrap_or(""), node["text"].as_str().unwrap_or(""));
                last["text"] = Value::String(joined);
                continue;
            }
        }
        merged.push(node);
    }
    merged
}

fn text_node(text: &str, marks: &[Value]) -> Value {
    let mut node = Map::new();
    node.insert("type".into(), json!("text"));
    node.insert("text".into(), json!(text));
    if !marks.is_empty() {
        node.insert("marks".into(), Value::Array(marks.to_vec()));
    }
    Value::Object(node)
}

fn inline_into(text: &str, marks: &[Value], tokens: &[Value], out: &mut Vec<Value>) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Value>| {
        if !plain.is_empty() {
            out.push(text_node(plain, marks));
            plain.clear();
        }
    };
    let with = |mark: Value| {
        let mut m = marks.to_vec();
        m.push(mark);
        m
    };
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        let rest = &text[at..];
        let next = chars.get(i + 1).map(|&(_, c)| c);

        // A delimited span: `(open, close, mark)`.
        let span = |open: &str, close: &str| -> Option<(usize, &str)> {
            let body_start = at + open.len();
            let end = text[body_start..].find(close)? + body_start;
            (end > body_start).then(|| (end + close.len(), &text[body_start..end]))
        };

        if c == '\\' && next.is_some() {
            plain.push(next.unwrap());
            i += 2;
            continue;
        }
        if c == '`' {
            if let Some((end, body)) = span("`", "`") {
                flush(&mut plain, out);
                out.push(text_node(body, &with(json!({"type": "code"}))));
                i = chars.iter().position(|&(b, _)| b >= end).unwrap_or(chars.len());
                continue;
            }
        }
        let delimited = [
            ("**", "**", json!({"type": "strong"})),
            ("~~", "~~", json!({"type": "strike"})),
        ];
        let mut handled = false;
        for (open, close, mark) in delimited {
            if rest.starts_with(open) {
                if let Some((end, body)) = span(open, close) {
                    flush(&mut plain, out);
                    inline_into(body, &with(mark), tokens, out);
                    i = chars.iter().position(|&(b, _)| b >= end).unwrap_or(chars.len());
                    handled = true;
                }
                break;
            }
        }
        if handled {
            continue;
        }
        let word_before = i.checked_sub(1).is_some_and(|j| chars[j].1.is_alphanumeric());
        if (c == '_' && !word_before) || (c == '*' && next != Some('*')) {
            let delim = if c == '_' { "_" } else { "*" };
            if let Some((end, body)) = span(delim, delim) {
                let after_ok = c != '_' || !text[end..].chars().next().is_some_and(char::is_alphanumeric);
                if after_ok {
                    flush(&mut plain, out);
                    inline_into(body, &with(json!({"type": "em"})), tokens, out);
                    i = chars.iter().position(|&(b, _)| b >= end).unwrap_or(chars.len());
                    continue;
                }
            }
        }
        if c == '[' {
            if let Some(close) = rest.find("](") {
                if let Some(paren) = rest[close + 2..].find(')') {
                    let label = &rest[1..close];
                    let href = &rest[close + 2..close + 2 + paren];
                    flush(&mut plain, out);
                    inline_into(label, &with(json!({"type": "link", "attrs": {"href": href}})), tokens, out);
                    let end = at + close + 2 + paren + 1;
                    i = chars.iter().position(|&(b, _)| b >= end).unwrap_or(chars.len());
                    continue;
                }
            }
        }
        if c == TOKEN_OPEN {
            if let Some((index, after)) = token_at(rest) {
                if let Some(node) = tokens.get(index) {
                    flush(&mut plain, out);
                    out.push(node.clone());
                    let end = text.len() - after.len();
                    i = chars.iter().position(|&(b, _)| b >= end).unwrap_or(chars.len());
                    continue;
                }
            }
        }
        plain.push(c);
        i += 1;
    }
    flush(&mut plain, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Value {
        json!({"type": "text", "text": t})
    }

    fn marked(t: &str, marks: Value) -> Value {
        json!({"type": "text", "text": t, "marks": marks})
    }

    fn para(content: Value) -> Value {
        json!({"type": "paragraph", "content": content})
    }

    fn doc(content: Value) -> Value {
        json!({"type": "doc", "version": 1, "content": content})
    }

    fn round_trip(original: &Value) -> (Markdown, Value) {
        let md = to_markdown(Some(original));
        let back = to_adf(&md.text, &md.tokens);
        (md, back)
    }

    #[test]
    fn rich_document_round_trips_exactly() {
        let original = doc(json!([
            {"type": "heading", "attrs": {"level": 2}, "content": [text("Context")]},
            para(json!([
                text("Plain, "),
                marked("bold", json!([{"type": "strong"}])),
                text(", "),
                marked("both", json!([{"type": "strong"}, {"type": "em"}])),
                text(", "),
                marked("code_here", json!([{"type": "code"}])),
                text(" and "),
                marked("a link", json!([{"type": "link", "attrs": {"href": "https://example.com"}}])),
            ])),
            {"type": "bulletList", "content": [
                {"type": "listItem", "content": [para(json!([text("one")]))]},
                {"type": "listItem", "content": [
                    para(json!([text("two")])),
                    {"type": "orderedList", "attrs": {"order": 3}, "content": [
                        {"type": "listItem", "content": [para(json!([text("nested three")]))]}
                    ]}
                ]}
            ]},
            {"type": "codeBlock", "attrs": {"language": "rust"}, "content": [text("let x = 1;\nlet y = x * 2;")]},
            {"type": "blockquote", "content": [para(json!([text("quoted")]))]},
            {"type": "rule"},
        ]));
        let (md, back) = round_trip(&original);
        assert_eq!(
            md.text,
            "## Context\n\nPlain, **bold**, **_both_**, `code_here` and [a link](https://example.com)\n\n\
             - one\n- two\n  3. nested three\n\n```rust\nlet x = 1;\nlet y = x * 2;\n```\n\n> quoted\n\n---"
        );
        assert_eq!(back, original);
        assert!(md.dropped.is_empty());
    }

    #[test]
    fn tables_and_mentions_survive_as_placeholders() {
        let mention = json!({"type": "mention", "attrs": {"id": "abc", "text": "@Riley Chen"}});
        let table = json!({"type": "table", "content": [{"type": "tableRow", "content": []}]});
        let original = doc(json!([para(json!([text("cc "), mention.clone(), text(" please")])), table.clone()]));
        let (md, back) = round_trip(&original);
        assert_eq!(md.text, "cc ⟦0:@Riley Chen⟧ please\n\n⟦1:table⟧");
        assert_eq!(md.placeholder_kinds(), ["mention", "table"]);
        assert_eq!(back, original);

        // Editing around the placeholders keeps them; deleting one drops it.
        let edited = to_adf("cc ⟦0:@Riley Chen⟧ now\n\nNew paragraph", &md.tokens);
        assert_eq!(edited["content"][0]["content"][1], mention);
        assert_eq!(edited["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn markup_characters_in_plain_text_are_escaped_and_restored() {
        let original = doc(json!([
            para(json!([text("PUBLIC.CHECK_STANDARD_ACH costs 2*3 [approx] ~~not strike~~ _x")])),
            para(json!([text("# not a heading")])),
            para(json!([text("1. not a list")])),
        ]));
        let (md, back) = round_trip(&original);
        assert!(md.text.contains("CHECK_STANDARD_ACH"), "identifiers stay readable: {}", md.text);
        assert_eq!(back, original);
    }

    #[test]
    fn headings_with_line_breaks_round_trip() {
        let original = doc(json!([
            {"type": "heading", "attrs": {"level": 3}, "content": [
                text("Issue Summary"), {"type": "hardBreak"}, {"type": "hardBreak"}, text("More")
            ]},
            {"type": "heading", "attrs": {"level": 1}, "content": [text("ends with a backslash \\")]},
        ]));
        let (md, back) = round_trip(&original);
        assert_eq!(md.text, "### Issue Summary\\\n\\\nMore\n\n# ends with a backslash \\\\");
        assert_eq!(back, original);
    }

    #[test]
    fn double_breaks_and_empty_paragraphs_round_trip() {
        let original = doc(json!([
            para(json!([text("a"), {"type": "hardBreak"}, {"type": "hardBreak"}, text("b")])),
            para(json!([])),
            para(json!([text("    indented: true")])),
            {"type": "heading", "attrs": {"level": 3}, "content": []},
        ]));
        let (_, back) = round_trip(&original);
        assert_eq!(back, original);
    }

    #[test]
    fn hard_breaks_are_newlines_and_unknown_marks_are_reported() {
        let original = doc(json!([para(json!([
            text("line one"),
            {"type": "hardBreak"},
            marked("underlined", json!([{"type": "underline"}])),
        ]))]));
        let md = to_markdown(Some(&original));
        assert_eq!(md.text, "line one\nunderlined");
        assert_eq!(md.dropped, ["underline"]);
    }

    #[test]
    fn plain_string_fields_from_the_board_api_are_editable() {
        let md = to_markdown(Some(&json!("Some *wiki* text")));
        let back = to_adf(&md.text, &md.tokens);
        assert_eq!(back, doc(json!([para(json!([text("Some *wiki* text")]))])));
    }

    #[test]
    fn markdown_typed_by_hand_parses() {
        let adf = to_adf("# Title\n\n* first\n* second\n\nSome *em* and **strong**.", &[]);
        assert_eq!(adf["content"][0]["type"], "heading");
        assert_eq!(adf["content"][1]["type"], "bulletList");
        assert_eq!(adf["content"][1]["content"].as_array().unwrap().len(), 2);
        assert_eq!(adf["content"][2]["content"][1]["marks"][0]["type"], "em");
        assert_eq!(adf["content"][2]["content"][3]["marks"][0]["type"], "strong");
    }

    #[test]
    fn empty_text_is_an_empty_document() {
        assert_eq!(to_adf("", &[]), doc(json!([])));
        assert_eq!(to_markdown(None).text, "");
    }
}

/// Round-trips real documents fetched from Jira. Run with
/// `JDO_RT_FILE=docs.json cargo test real_documents -- --ignored --nocapture`,
/// where the file is a `/search/jql` response.
#[cfg(test)]
mod real_documents {
    use super::*;

    #[test]
    #[ignore]
    fn real_documents_round_trip() {
        let path = std::env::var("JDO_RT_FILE").expect("JDO_RT_FILE");
        let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (mut exact, mut lossy, mut different) = (0, 0, Vec::new());
        for issue in data["issues"].as_array().unwrap() {
            for field in ["description", "customfield_10035"] {
                let doc = &issue["fields"][field];
                if doc.is_null() {
                    continue;
                }
                let md = to_markdown(Some(doc));
                let back = to_adf(&md.text, &md.tokens);
                let (doc, back) = (&normalize(doc), &normalize(&back));
                if back == doc {
                    exact += 1;
                } else if !md.dropped.is_empty() {
                    lossy += 1;
                    println!("{} {field}: drops {:?}", issue["key"], md.dropped);
                } else {
                    if different.len() < 12 {
                        println!("DIFF {} {field}: {}", issue["key"], first_diff(doc, back, "$"));
                    }
                    different.push(format!("{} {field}", issue["key"]));
                }
            }
        }
        println!("exact: {exact}, lossy (unsupported marks): {lossy}, different: {}", different.len());
        assert!(different.is_empty(), "changed without a reported loss: {different:?}");
    }

    /// Drop differences that carry no content: editor-assigned `localId`s,
    /// line breaks at a paragraph's end, whitespace at its edges, and how text
    /// is split into nodes.
    fn normalize(v: &Value) -> Value {
        match v {
            Value::Object(map) => {
                let mut out = Map::new();
                for (k, val) in map {
                    if k == "attrs" {
                        let mut attrs = val.as_object().cloned().unwrap_or_default();
                        attrs.remove("localId");
                        if attrs.get("order") == Some(&json!(1)) {
                            attrs.remove("order");
                        }
                        if !attrs.is_empty() {
                            out.insert(k.clone(), normalize(&Value::Object(attrs)));
                        }
                    } else {
                        out.insert(k.clone(), normalize(val));
                    }
                }
                let kind = out.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                if matches!(kind.as_str(), "paragraph" | "heading") && !out.contains_key("content") {
                    out.insert("content".into(), json!([]));
                }
                if kind == "paragraph" {
                    let content: Vec<Value> = out.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
                    out.insert("content".into(), Value::Array(tidy_inline(content)));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
            other => other.clone(),
        }
    }

    fn tidy_inline(nodes: Vec<Value>) -> Vec<Value> {
        // A raw newline inside a text node displays as a line break.
        let mut split = Vec::new();
        for node in nodes {
            match node["text"].as_str().filter(|t| node_type(&node) == "text" && t.contains('\n')) {
                Some(text) => {
                    for (i, part) in text.split('\n').enumerate() {
                        if i > 0 {
                            split.push(json!({"type": "hardBreak"}));
                        }
                        let mut piece = node.clone();
                        piece["text"] = json!(part);
                        split.push(piece);
                    }
                }
                None => split.push(node),
            }
        }
        let mut merged: Vec<Value> = Vec::new();
        for node in split {
            if let (Some(last), "text") = (merged.last_mut(), node_type(&node)) {
                if node_type(last) == "text" && last.get("marks") == node.get("marks") {
                    let joined = format!("{}{}", last["text"].as_str().unwrap_or(""), node["text"].as_str().unwrap_or(""));
                    last["text"] = Value::String(joined);
                    continue;
                }
            }
            merged.push(node);
        }
        if let Some(first) = merged.first_mut().filter(|n| node_type(n) == "text") {
            first["text"] = Value::String(first["text"].as_str().unwrap_or("").trim_start().to_string());
        }
        loop {
            if let Some(last) = merged.last_mut().filter(|n| node_type(n) == "text") {
                last["text"] = Value::String(last["text"].as_str().unwrap_or("").trim_end().to_string());
            }
            merged.retain(|n| node_type(n) != "text" || !n["text"].as_str().unwrap_or("").is_empty());
            if merged.last().is_some_and(|n| node_type(n) == "hardBreak") {
                merged.pop();
            } else {
                break merged;
            }
        }
    }

    fn first_diff(a: &Value, b: &Value, path: &str) -> String {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                for key in x.keys().chain(y.keys()) {
                    if x.get(key) != y.get(key) {
                        return first_diff(x.get(key).unwrap_or(&Value::Null), y.get(key).unwrap_or(&Value::Null), &format!("{path}.{key}"));
                    }
                }
                format!("{path}: objects equal?")
            }
            (Value::Array(x), Value::Array(y)) => {
                for i in 0..x.len().max(y.len()) {
                    if x.get(i) != y.get(i) {
                        return first_diff(x.get(i).unwrap_or(&Value::Null), y.get(i).unwrap_or(&Value::Null), &format!("{path}[{i}]"));
                    }
                }
                format!("{path}: arrays equal?")
            }
            _ => format!("{path}: {} → {}", short(a), short(b)),
        }
    }

    fn short(v: &Value) -> String {
        let s = v.to_string();
        if s.chars().count() > 90 { format!("{}…", s.chars().take(90).collect::<String>()) } else { s }
    }
}
