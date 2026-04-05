//! TypeScript and JavaScript language chunker.
//!
//! Splits TS/JS source files on top-level declarations (function, class,
//! interface, type, enum, const, let, var) using regex detection and
//! brace-depth tracking to find item boundaries.
//!
//! Both `chunk_typescript` and `chunk_javascript` delegate to the shared
//! internal `chunk_ts_or_js` with different language tags.

use crate::chunker::{split_on_blank_lines, ChunkMeta};
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

fn item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:export\s+)?(?:default\s+)?(?:async\s+)?(function|class|interface|type|enum|const|let|var)\s+(\w+)",
        )
        .expect("valid ts item regex")
    })
}

fn make_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Chunk a TypeScript source file by top-level declarations.
pub fn chunk_typescript(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    chunk_ts_or_js(source_path, content, max_tokens, "typescript")
}

/// Chunk a JavaScript source file by top-level declarations.
pub fn chunk_javascript(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    chunk_ts_or_js(source_path, content, max_tokens, "javascript")
}

fn chunk_ts_or_js(
    source_path: &str,
    content: &str,
    max_tokens: usize,
    language: &str,
) -> Vec<ChunkMeta> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let max_chars = max_tokens * 4;

    let items = find_top_level_items(&lines);
    let mut chunks = Vec::new();
    let mut chunk_index = 0;

    // Emit prelude (lines before first item) if non-trivial
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
                content_hash: make_hash(&prelude),
                language: Some(language.to_string()),
                symbol: Some("<prelude>".to_string()),
                signature: lines.first().map(|s| s.to_string()),
                line_range: Some((1, prelude_end)),
            });
            chunk_index += 1;
        }
    }

    for item in items {
        let end_line = item.end_line.min(lines.len() - 1);
        let item_content: String = lines[item.start_line..=end_line].join("\n");
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
                content_hash: make_hash(&item_content),
                language: Some(language.to_string()),
                symbol: Some(item.symbol.clone()),
                signature: Some(signature.trim().to_string()),
                line_range: Some((item.start_line + 1, end_line + 1)),
            });
            chunk_index += 1;
        } else {
            // Large item: split on blank lines
            let sub_chunks = split_on_blank_lines(&item_content, max_chars);
            for (sub_idx, sub) in sub_chunks.into_iter().enumerate() {
                chunks.push(ChunkMeta {
                    id: format!("{source_path}:{chunk_index}"),
                    source_path: source_path.to_string(),
                    chunk_index,
                    content: sub.clone(),
                    heading_hierarchy: Vec::new(),
                    byte_offset: 0,
                    byte_length: sub.len(),
                    content_hash: make_hash(&sub),
                    language: Some(language.to_string()),
                    symbol: Some(format!("{} (part {})", item.symbol, sub_idx + 1)),
                    signature: Some(signature.trim().to_string()),
                    line_range: Some((item.start_line + 1, end_line + 1)),
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

/// Find all top-level items using brace-depth tracking.
fn find_top_level_items(lines: &[&str]) -> Vec<TopLevelItem> {
    let mut items = Vec::new();
    let mut brace_depth: i32 = 0;
    let mut current_item_start: Option<usize> = None;
    let mut current_symbol = String::new();
    let re = item_regex();
    let mut in_block_comment = false;

    for (i, line) in lines.iter().enumerate() {
        let stripped = strip_comments(line, &mut in_block_comment);

        // At depth 0 and not tracking an item — check if this starts one
        if brace_depth == 0 && current_item_start.is_none() {
            let trimmed = stripped.trim_start();
            if let Some(cap) = re.captures(trimmed) {
                let kind = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let name = cap.get(2).map(|m| m.as_str()).unwrap_or("");
                current_symbol = format!("{kind} {name}");
                current_item_start = Some(i);
            }
        }

        // Update brace depth
        brace_depth += count_braces(&stripped);

        // Check for item completion
        if let Some(start) = current_item_start {
            let line_has_open = stripped.contains('{');
            let line_has_semi = stripped.trim_end().ends_with(';');

            if brace_depth == 0 && (line_has_open || start != i) {
                // Block item completed (depth back to 0 after being positive)
                items.push(TopLevelItem {
                    start_line: start,
                    end_line: i,
                    symbol: current_symbol.clone(),
                });
                current_item_start = None;
                current_symbol.clear();
            } else if brace_depth == 0 && line_has_semi && !line_has_open && start == i {
                // Single-line declaration terminated by semicolon (const, type alias, etc.)
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

    // If we're still inside an item at EOF (e.g. file without trailing newline), emit it
    if let Some(start) = current_item_start {
        items.push(TopLevelItem {
            start_line: start,
            end_line: lines.len().saturating_sub(1),
            symbol: current_symbol,
        });
    }

    items
}

/// Strip line comments (`//`) and block comments (`/* */`).
/// Does NOT handle string contents — that's handled separately in `count_braces`.
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

/// Count net brace depth change for a line, skipping string and template literal contents.
fn count_braces(line: &str) -> i32 {
    let mut depth: i32 = 0;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut in_template = false;
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

        // Toggle string/template state
        if c == '"' && !in_single_quote && !in_template {
            in_double_quote = !in_double_quote;
            continue;
        }
        if c == '\'' && !in_double_quote && !in_template {
            in_single_quote = !in_single_quote;
            continue;
        }
        if c == '`' && !in_double_quote && !in_single_quote {
            in_template = !in_template;
            continue;
        }

        if in_double_quote || in_single_quote || in_template {
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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TS: &str = r#"
// A sample TypeScript file.

import { foo } from './foo';

export const MAX = 100;

export interface User {
    id: string;
    name: string;
}

export type Handler = (u: User) => void;

export function parseUser(input: string): User {
    return JSON.parse(input);
}

export class UserService {
    constructor(private db: Database) {}

    async findById(id: string): Promise<User | null> {
        return this.db.query(id);
    }
}

export const handler: Handler = (u) => {
    console.log(u.name);
};
"#;

    #[test]
    fn test_chunk_typescript_finds_declarations() {
        let chunks = chunk_typescript("test.ts", SAMPLE_TS, 1000);
        assert!(
            chunks.len() >= 4,
            "expected >= 4 chunks, got {}",
            chunks.len()
        );
        let symbols: Vec<String> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(
            symbols.iter().any(|s| s.contains("User")),
            "should find User, got {:?}",
            symbols
        );
        assert!(
            symbols.iter().any(|s| s.contains("parseUser")),
            "should find parseUser, got {:?}",
            symbols
        );
        assert!(
            symbols.iter().any(|s| s.contains("UserService")),
            "should find UserService, got {:?}",
            symbols
        );
    }

    #[test]
    fn test_chunk_typescript_attaches_language() {
        let chunks = chunk_typescript("test.ts", SAMPLE_TS, 1000);
        for c in &chunks {
            assert_eq!(c.language.as_deref(), Some("typescript"));
        }
    }

    #[test]
    fn test_chunk_javascript_uses_different_language_tag() {
        let chunks = chunk_javascript("test.js", "function foo() { return 1; }", 1000);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language.as_deref(), Some("javascript"));
    }

    #[test]
    fn test_chunk_typescript_empty_file() {
        let chunks = chunk_typescript("empty.ts", "", 1000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_typescript_template_literals_not_counted_as_braces() {
        // Template literal with braces — should not confuse brace depth
        let src =
            "export function greet(name: string): string {\n    return `Hello, ${name}!`;\n}\n";
        let chunks = chunk_typescript("test.ts", src, 1000);
        assert_eq!(chunks.len(), 1, "should produce exactly 1 chunk, not split");
        assert_eq!(chunks[0].symbol.as_deref(), Some("function greet"));
    }

    #[test]
    fn test_chunk_typescript_const_single_line() {
        let src = "export const MAX = 100;\nexport const MIN = 0;\n";
        let chunks = chunk_typescript("test.ts", src, 1000);
        // Each const should be its own chunk (both single-line semicolon-terminated)
        assert!(
            chunks.len() >= 2,
            "expected >= 2 chunks, got {}",
            chunks.len()
        );
        let symbols: Vec<String> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(symbols.iter().any(|s| s.contains("MAX")));
        assert!(symbols.iter().any(|s| s.contains("MIN")));
    }

    #[test]
    fn test_chunk_typescript_line_range_is_1indexed() {
        let src = "export function foo() {\n    return 1;\n}\n";
        let chunks = chunk_typescript("test.ts", src, 1000);
        let foo = chunks
            .iter()
            .find(|c| {
                c.symbol
                    .as_deref()
                    .map(|s| s.contains("foo"))
                    .unwrap_or(false)
            })
            .expect("should find foo");
        let (start, _) = foo.line_range.unwrap();
        assert_eq!(start, 1, "line_range should be 1-indexed");
    }
}
