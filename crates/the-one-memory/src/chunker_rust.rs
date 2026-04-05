//! Rust language chunker.
//!
//! Splits Rust source files on top-level items (fn, struct, enum, impl, trait,
//! mod, type, const, static, macro_rules!) using regex detection and
//! brace-depth tracking to find item boundaries.

use crate::chunker::ChunkMeta;
use regex::Regex;
use std::sync::OnceLock;

/// Matches the start of a Rust top-level item.
///
/// Captures the optional visibility and `async` modifiers, then the item kind
/// and name. Does NOT match items inside other items (we track brace depth for that).
fn item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:default\s+)?(fn|struct|enum|impl|trait|mod|type|const|static|macro_rules!)\b",
        )
        .expect("valid rust item regex")
    })
}

/// Chunk a Rust source file by top-level items.
///
/// Each top-level item becomes a chunk with `symbol`, `signature`, and
/// `line_range` metadata. Items larger than `max_tokens` are split on blank
/// lines within the item as a fallback.
pub fn chunk_rust(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    // Estimate 4 chars per token for content length limit
    let max_chars = max_tokens * 4;

    let items = find_top_level_items(&lines);
    let mut chunks = Vec::new();
    let mut chunk_index = 0;

    // Emit a chunk for the "prelude" (lines before the first item) — imports, attributes, etc.
    let prelude_end = items.first().map(|it| it.start_line).unwrap_or(lines.len());
    if prelude_end > 0 {
        let prelude: String = lines[..prelude_end].join("\n");
        if !prelude.trim().is_empty() {
            chunks.push(ChunkMeta {
                id: format!("{source_path}:{chunk_index}"),
                source_path: source_path.to_string(),
                chunk_index,
                content: prelude.trim_end().to_string(),
                heading_hierarchy: Vec::new(),
                byte_offset: 0,
                byte_length: prelude.len(),
                content_hash: {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    prelude.hash(&mut hasher);
                    format!("{:x}", hasher.finish())
                },
                language: Some("rust".to_string()),
                symbol: Some("<prelude>".to_string()),
                signature: lines.first().map(|s| s.to_string()),
                line_range: Some((1, prelude_end)),
            });
            chunk_index += 1;
        }
    }

    for item in items {
        let item_lines = &lines[item.start_line..=item.end_line.min(lines.len() - 1)];
        let item_content: String = item_lines.join("\n");
        let signature = lines.get(item.start_line).unwrap_or(&"").to_string();

        if item_content.len() <= max_chars {
            chunks.push(ChunkMeta {
                id: format!("{source_path}:{chunk_index}"),
                source_path: source_path.to_string(),
                chunk_index,
                content: item_content.clone(),
                heading_hierarchy: Vec::new(),
                byte_offset: 0,
                byte_length: item_content.len(),
                content_hash: {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    item_content.hash(&mut hasher);
                    format!("{:x}", hasher.finish())
                },
                language: Some("rust".to_string()),
                symbol: Some(item.symbol.clone()),
                signature: Some(signature.trim().to_string()),
                line_range: Some((item.start_line + 1, item.end_line + 1)),
            });
            chunk_index += 1;
        } else {
            // Large item: split on blank lines
            let mut sub_chunks = crate::chunker::split_on_blank_lines(&item_content, max_chars);
            for (sub_idx, sub) in sub_chunks.drain(..).enumerate() {
                chunks.push(ChunkMeta {
                    id: format!("{source_path}:{chunk_index}"),
                    source_path: source_path.to_string(),
                    chunk_index,
                    content: sub.clone(),
                    heading_hierarchy: Vec::new(),
                    byte_offset: 0,
                    byte_length: sub.len(),
                    content_hash: {
                        use std::collections::hash_map::DefaultHasher;
                        use std::hash::{Hash, Hasher};
                        let mut hasher = DefaultHasher::new();
                        sub.hash(&mut hasher);
                        format!("{:x}", hasher.finish())
                    },
                    language: Some("rust".to_string()),
                    symbol: Some(format!("{} (part {})", item.symbol, sub_idx + 1)),
                    signature: Some(signature.trim().to_string()),
                    line_range: Some((item.start_line + 1, item.end_line + 1)),
                });
                chunk_index += 1;
            }
        }
    }

    chunks
}

#[derive(Debug)]
struct TopLevelItem {
    start_line: usize, // 0-indexed
    end_line: usize,   // 0-indexed, inclusive
    symbol: String,
}

/// Find all top-level items (brace depth 0) in the lines.
fn find_top_level_items(lines: &[&str]) -> Vec<TopLevelItem> {
    let mut items = Vec::new();
    let mut brace_depth: i32 = 0;
    let mut current_item_start: Option<usize> = None;
    let mut current_symbol: String = String::new();
    let re = item_regex();
    let mut in_block_comment = false;

    for (i, line) in lines.iter().enumerate() {
        // Strip line comments and block comments (simple pass)
        let stripped = strip_comments(line, &mut in_block_comment);

        // If at depth 0 and not in an item, check if this line starts an item
        if brace_depth == 0 && current_item_start.is_none() {
            let trimmed = stripped.trim_start();
            if let Some(cap) = re.captures(trimmed) {
                let kind = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let symbol = extract_rust_symbol(trimmed, kind);
                current_item_start = Some(i);
                current_symbol = symbol;
            }
        }

        // Update brace depth based on this line
        brace_depth += count_braces(&stripped);

        // If we're inside an item and brace_depth returns to 0 AFTER going positive,
        // or if we're on a semicolon-terminated item at depth 0, emit it
        if let Some(start) = current_item_start {
            let line_has_open = stripped.contains('{');
            let line_has_semi = stripped.trim_end().ends_with(';');

            if brace_depth == 0 && (line_has_open || start != i) {
                // Block item completed (depth back to 0 after entering)
                items.push(TopLevelItem {
                    start_line: start,
                    end_line: i,
                    symbol: current_symbol.clone(),
                });
                current_item_start = None;
                current_symbol.clear();
            } else if brace_depth == 0 && line_has_semi && !line_has_open && start == i {
                // Semicolon-terminated item (const, type alias, use, static, extern)
                items.push(TopLevelItem {
                    start_line: start,
                    end_line: i,
                    symbol: current_symbol.clone(),
                });
                current_item_start = None;
                current_symbol.clear();
            }
        }
    }

    items
}

fn strip_comments(line: &str, in_block: &mut bool) -> String {
    let mut result = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if *in_block {
            if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '/' {
                *in_block = false;
                i += 2;
            } else {
                i += 1;
            }
        } else if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
            break; // rest of line is a line comment
        } else if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
            *in_block = true;
            i += 2;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn count_braces(line: &str) -> i32 {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut in_char = false;
    let mut prev_backslash = false;
    for c in line.chars() {
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if c == '\\' {
            prev_backslash = true;
            continue;
        }
        if c == '"' && !in_char {
            in_string = !in_string;
            continue;
        }
        if c == '\'' && !in_string {
            in_char = !in_char;
            continue;
        }
        if in_string || in_char {
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn extract_rust_symbol(line: &str, kind: &str) -> String {
    // Extract the item name after the kind keyword. E.g.
    // "fn parse_config(...)" -> "fn parse_config"
    // "impl<T> MyTrait for MyType<T>" -> "impl MyTrait for MyType"
    // "struct Broker {" -> "struct Broker"
    let after_kind = line.split_once(kind).map(|(_, r)| r).unwrap_or("").trim();
    let name_end = after_kind
        .find(['(', '{', '<', '=', ':', ';', ' '])
        .unwrap_or(after_kind.len());
    let name = &after_kind[..name_end];
    if kind == "impl" {
        // Try to capture the "impl X for Y" pattern
        if let Some(for_idx) = after_kind.find(" for ") {
            let after_for = &after_kind[for_idx + 5..];
            let after_for_end = after_for
                .find(['<', '{', ' ', ','])
                .unwrap_or(after_for.len());
            return format!(
                "impl {} for {}",
                after_kind[..for_idx].trim(),
                after_for[..after_for_end].trim()
            );
        }
    }
    format!("{kind} {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
//! A sample Rust file.

use std::collections::HashMap;

const MAX: usize = 100;

pub fn parse_config(input: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in input.lines() {
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.to_string(), v.to_string());
        }
    }
    out
}

pub struct Broker {
    pub name: String,
    count: usize,
}

impl Broker {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), count: 0 }
    }
}

pub trait Handler {
    fn handle(&self);
}
"#;

    #[test]
    fn test_chunk_rust_finds_top_level_items() {
        let chunks = chunk_rust("test.rs", SAMPLE, 1000);
        // Expect: prelude, const MAX, fn parse_config, struct Broker, impl Broker, trait Handler = 6 chunks
        assert!(
            chunks.len() >= 4,
            "expected at least 4 chunks, got {}",
            chunks.len()
        );

        let symbols: Vec<&str> = chunks.iter().filter_map(|c| c.symbol.as_deref()).collect();

        assert!(
            symbols.iter().any(|s| s.contains("parse_config")),
            "should find parse_config, got {:?}",
            symbols
        );
        assert!(
            symbols.iter().any(|s| s.contains("Broker")),
            "should find Broker struct, got {:?}",
            symbols
        );
    }

    #[test]
    fn test_chunk_rust_attaches_language_metadata() {
        let chunks = chunk_rust("test.rs", SAMPLE, 1000);
        for chunk in &chunks {
            assert_eq!(chunk.language.as_deref(), Some("rust"));
            assert!(chunk.line_range.is_some());
        }
    }

    #[test]
    fn test_chunk_rust_attaches_signatures() {
        let chunks = chunk_rust("test.rs", SAMPLE, 1000);
        let fn_chunk = chunks
            .iter()
            .find(|c| {
                c.symbol
                    .as_deref()
                    .map(|s| s.contains("parse_config"))
                    .unwrap_or(false)
            })
            .expect("should find parse_config chunk");

        let sig = fn_chunk.signature.as_deref().unwrap_or("");
        assert!(
            sig.contains("parse_config"),
            "signature should contain function name, got: {sig}"
        );
    }

    #[test]
    fn test_chunk_rust_empty_file() {
        let chunks = chunk_rust("empty.rs", "", 1000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_rust_brace_counting_ignores_strings() {
        let content = r#"
fn test() {
    let s = "hello { world }";
    let c = '{';
}
"#;
        let chunks = chunk_rust("test.rs", content, 1000);
        assert_eq!(
            chunks.len(),
            1,
            "brace tracking should handle strings/chars"
        );
        assert_eq!(chunks[0].symbol.as_deref(), Some("fn test"));
    }
}
