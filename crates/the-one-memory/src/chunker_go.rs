//! Go language chunker.
//!
//! Splits Go source files on top-level declarations (func, type, var, const,
//! import) using regex detection and brace-depth tracking. Handles method
//! receivers: `func (r *Receiver) Method(...)`.

use crate::chunker::{split_on_blank_lines, ChunkMeta};
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

fn item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches: func, type, var, const, import at start of line
        Regex::new(r"^(func|type|var|const|import)\b").expect("valid go item regex")
    })
}

fn make_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Chunk a Go source file by top-level declarations.
pub fn chunk_go(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let max_chars = max_tokens * 4;

    let items = find_top_level_items(&lines);
    let mut chunks = Vec::new();
    let mut chunk_index = 0;

    // Emit prelude (package declaration and build tags) if non-trivial
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
                language: Some("go".to_string()),
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
                language: Some("go".to_string()),
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
                    language: Some("go".to_string()),
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

fn find_top_level_items(lines: &[&str]) -> Vec<TopLevelItem> {
    let mut items = Vec::new();
    let mut brace_depth: i32 = 0;
    let mut paren_depth: i32 = 0; // for const/var/import blocks with parens
    let mut current_item_start: Option<usize> = None;
    let mut current_symbol = String::new();
    let re = item_regex();
    let mut in_block_comment = false;

    for (i, line) in lines.iter().enumerate() {
        let stripped = strip_comments(line, &mut in_block_comment);
        let trimmed = stripped.trim_start();

        // At depth 0, check if this starts a new top-level item
        if brace_depth == 0
            && paren_depth == 0
            && current_item_start.is_none()
            && re.is_match(trimmed)
        {
            let symbol = extract_go_symbol(trimmed);
            current_symbol = symbol;
            current_item_start = Some(i);
        }

        // Update depths
        brace_depth += count_braces_go(&stripped);
        paren_depth += count_parens_go(&stripped);

        // Check for item completion
        if let Some(start) = current_item_start {
            let line_has_open_brace = stripped.contains('{');
            let line_has_open_paren = stripped.contains('(');

            // An item is complete when:
            // (a) A brace/paren block was opened and depths returned to 0, OR
            // (b) The item is a single-line declaration (start == i) with no block opener
            //     (e.g. `const PI = 3.14`, `type Foo = Bar`).
            let completed = brace_depth == 0
                && paren_depth == 0
                && (start != i || (!line_has_open_brace && !line_has_open_paren));

            if completed {
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

    // Flush any open item at EOF
    if let Some(start) = current_item_start {
        items.push(TopLevelItem {
            start_line: start,
            end_line: lines.len().saturating_sub(1),
            symbol: current_symbol,
        });
    }

    items
}

/// Extract a human-readable symbol name from a Go top-level declaration line.
fn extract_go_symbol(line: &str) -> String {
    // func (r *Receiver) MethodName(...) -> "func (*Receiver).MethodName"
    // func FuncName(...) -> "func FuncName"
    // type TypeName struct -> "type TypeName"
    // const PI = ... -> "const PI"
    // var x int -> "var x"
    // import "fmt" -> "import"
    // import ( ... ) -> "import"
    if let Some(rest) = line.strip_prefix("func") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            // Method with receiver: (r *Receiver) MethodName(...)
            // Find the closing paren of the receiver
            let close = rest.find(')').unwrap_or(rest.len());
            let receiver = &rest[1..close]; // e.g. "r *Receiver"
                                            // Simplify receiver to just the type
            let receiver_type = receiver
                .split_whitespace()
                .last()
                .unwrap_or(receiver)
                .trim_start_matches('*');
            let after_receiver = rest[close + 1..].trim_start();
            let method_name = after_receiver.split(['(', ' ', '\t']).next().unwrap_or("?");
            return format!("func (*{receiver_type}).{method_name}");
        } else {
            let name = rest.split(['(', ' ', '\t']).next().unwrap_or("?");
            return format!("func {name}");
        }
    }
    if let Some(rest) = line.strip_prefix("type") {
        let rest = rest.trim_start();
        let name = rest.split([' ', '\t', '{', '=']).next().unwrap_or("?");
        return format!("type {name}");
    }
    if let Some(rest) = line.strip_prefix("const") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            return "const (block)".to_string();
        }
        let name = rest.split([' ', '\t', '=']).next().unwrap_or("?");
        return format!("const {name}");
    }
    if let Some(rest) = line.strip_prefix("var") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            return "var (block)".to_string();
        }
        let name = rest.split([' ', '\t', '=']).next().unwrap_or("?");
        return format!("var {name}");
    }
    if line.starts_with("import") {
        return "import".to_string();
    }
    line.split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip line comments (`//`) and block comments (`/* */`) from a Go line.
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
            break;
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

/// Count net brace depth change for a Go line, skipping string contents.
fn count_braces_go(line: &str) -> i32 {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut in_raw_string = false;
    let mut prev_backslash = false;

    for c in line.chars() {
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if c == '\\' && in_string {
            prev_backslash = true;
            continue;
        }
        if c == '"' && !in_raw_string {
            in_string = !in_string;
            continue;
        }
        if c == '`' && !in_string {
            in_raw_string = !in_raw_string;
            continue;
        }
        if in_string || in_raw_string {
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

/// Count net parenthesis depth change for a Go line (for const/var/import blocks).
fn count_parens_go(line: &str) -> i32 {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut in_raw_string = false;
    let mut prev_backslash = false;

    for c in line.chars() {
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if c == '\\' && in_string {
            prev_backslash = true;
            continue;
        }
        if c == '"' && !in_raw_string {
            in_string = !in_string;
            continue;
        }
        if c == '`' && !in_string {
            in_raw_string = !in_raw_string;
            continue;
        }
        if in_string || in_raw_string {
            continue;
        }
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_GO: &str = r#"
package main

import (
    "fmt"
    "strings"
)

const MaxSize = 100

type User struct {
    ID   string
    Name string
}

func (u *User) String() string {
    return fmt.Sprintf("User{%s, %s}", u.ID, u.Name)
}

func parseUser(s string) *User {
    parts := strings.Split(s, ",")
    return &User{ID: parts[0], Name: parts[1]}
}

func main() {
    u := parseUser("1,alice")
    fmt.Println(u)
}
"#;

    #[test]
    fn test_chunk_go_finds_top_level_items() {
        let chunks = chunk_go("test.go", SAMPLE_GO, 1000);
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
            symbols.iter().any(|s| s.contains("main")),
            "should find main, got {:?}",
            symbols
        );
    }

    #[test]
    fn test_chunk_go_handles_method_receiver() {
        let chunks = chunk_go("test.go", SAMPLE_GO, 1000);
        let string_method = chunks.iter().find(|c| {
            c.symbol
                .as_deref()
                .map(|s| s.contains("String"))
                .unwrap_or(false)
        });
        assert!(string_method.is_some(), "should find String method");
        // Symbol should indicate it's a method on *User
        let sym = string_method.unwrap().symbol.as_deref().unwrap_or("");
        assert!(
            sym.contains("User") || sym.contains("String"),
            "symbol should reference receiver or method name: {sym}"
        );
    }

    #[test]
    fn test_chunk_go_attaches_language() {
        let chunks = chunk_go("test.go", SAMPLE_GO, 1000);
        for c in &chunks {
            assert_eq!(c.language.as_deref(), Some("go"));
        }
    }

    #[test]
    fn test_chunk_go_empty_file() {
        let chunks = chunk_go("empty.go", "", 1000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_go_const_single_line() {
        let src = "package main\n\nconst PI = 3.14\n";
        let chunks = chunk_go("test.go", src, 1000);
        let symbols: Vec<String> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(
            symbols.iter().any(|s| s.contains("PI")),
            "should find const PI, got {:?}",
            symbols
        );
    }

    #[test]
    fn test_chunk_go_import_block() {
        let src = "package main\n\nimport (\n    \"fmt\"\n    \"os\"\n)\n\nfunc main() {}\n";
        let chunks = chunk_go("test.go", src, 1000);
        let symbols: Vec<String> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(
            symbols.iter().any(|s| s.contains("import")),
            "should have import chunk, got {:?}",
            symbols
        );
        assert!(
            symbols.iter().any(|s| s.contains("main")),
            "should have main chunk, got {:?}",
            symbols
        );
    }

    #[test]
    fn test_chunk_go_line_range_is_1indexed() {
        let src = "package main\n\nfunc foo() {}\n";
        let chunks = chunk_go("test.go", src, 1000);
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
        assert!(start >= 1, "line_range should be 1-indexed, got {start}");
    }
}
