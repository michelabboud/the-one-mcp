//! Python language chunker.
//!
//! Splits Python source files on top-level `def`, `async def`, and `class`
//! items using regex detection and indentation tracking.

use crate::chunker::{split_on_blank_lines, ChunkMeta};
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

fn item_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(async\s+def|def|class)\s+(\w+)").expect("valid python item regex")
    })
}

fn decorator_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^@\w").expect("valid decorator regex"))
}

fn make_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Chunk a Python source file by top-level items.
pub fn chunk_python(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
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
                language: Some("python".to_string()),
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
                language: Some("python".to_string()),
                symbol: Some(item.symbol.clone()),
                signature: Some(signature.trim().to_string()),
                line_range: Some((item.start_line + 1, end_line + 1)),
            });
            chunk_index += 1;
        } else {
            // Large item: split on blank lines within the item
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
                    language: Some("python".to_string()),
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
    start_line: usize,
    end_line: usize,
    symbol: String,
}

fn find_top_level_items(lines: &[&str]) -> Vec<TopLevelItem> {
    let mut items = Vec::new();
    let re = item_regex();
    let dec_re = decorator_regex();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Skip blank lines
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // Only process top-level lines (indent == 0)
        if indent_of(line) > 0 {
            i += 1;
            continue;
        }

        // Check for decorator(s) preceding an item
        let decorator_start = if dec_re.is_match(line) {
            let dec_start = i;
            while i < lines.len() && !lines[i].trim().is_empty() && dec_re.is_match(lines[i]) {
                i += 1;
            }
            // skip blank lines between decorators and def/class
            while i < lines.len() && lines[i].trim().is_empty() {
                i += 1;
            }
            Some(dec_start)
        } else {
            None
        };

        if i >= lines.len() {
            break;
        }

        let current_line = lines[i];

        // After decorator, make sure we're still at indent 0
        if indent_of(current_line) > 0 {
            i += 1;
            continue;
        }

        if let Some(cap) = re.captures(current_line) {
            let kind = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let name = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            // Normalize "async def" -> "async def", "def" -> "def", "class" -> "class"
            let symbol = format!("{kind} {name}");
            let start_line = decorator_start.unwrap_or(i);
            let def_line = i;

            // Find end of item: next non-blank line at indent 0 that is NOT a continuation
            let mut end = def_line;
            let mut j = def_line + 1;
            while j < lines.len() {
                let next = lines[j];
                if next.trim().is_empty() {
                    j += 1;
                    continue;
                }
                let next_indent = indent_of(next);
                if next_indent == 0 {
                    // Reached a new top-level construct
                    break;
                }
                end = j;
                j += 1;
            }

            items.push(TopLevelItem {
                start_line,
                end_line: end,
                symbol,
            });
            i = j;
        } else {
            // Top-level non-item line (e.g. import, constant assignment, etc.)
            // If we had collected a decorator_start but the next line isn't a def/class,
            // that decorator was for something we skip — just advance.
            i += 1;
        }
    }

    items
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
"""Module docstring."""

import os
from typing import List

CONSTANT = 42

def hello(name: str) -> str:
    """Greet a name."""
    return f"Hello, {name}!"

class Greeter:
    def __init__(self, prefix: str):
        self.prefix = prefix

    def greet(self, name: str) -> str:
        return f"{self.prefix} {name}"

@staticmethod
def utility():
    pass
"#;

    #[test]
    fn test_chunk_python_finds_top_level_items() {
        let chunks = chunk_python("test.py", SAMPLE, 1000);
        assert!(
            chunks.len() >= 3,
            "expected >= 3 chunks, got {}",
            chunks.len()
        );

        let symbols: Vec<String> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(
            symbols.iter().any(|s| s.contains("hello")),
            "should find hello, got {:?}",
            symbols
        );
        assert!(
            symbols.iter().any(|s| s.contains("Greeter")),
            "should find Greeter, got {:?}",
            symbols
        );
    }

    #[test]
    fn test_chunk_python_attaches_language() {
        let chunks = chunk_python("test.py", SAMPLE, 1000);
        for c in &chunks {
            assert_eq!(c.language.as_deref(), Some("python"));
        }
    }

    #[test]
    fn test_chunk_python_handles_decorators() {
        let chunks = chunk_python("test.py", SAMPLE, 1000);
        let util = chunks.iter().find(|c| {
            c.symbol
                .as_deref()
                .map(|s| s.contains("utility"))
                .unwrap_or(false)
        });
        assert!(
            util.is_some(),
            "should find utility function with decorator"
        );
        // The chunk should include the @staticmethod line
        assert!(
            util.unwrap().content.contains("@staticmethod"),
            "chunk should contain the decorator"
        );
    }

    #[test]
    fn test_chunk_python_empty_file() {
        let chunks = chunk_python("empty.py", "", 1000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_python_async_def() {
        let src = "async def fetch(url: str):\n    pass\n";
        let chunks = chunk_python("test.py", src, 1000);
        assert!(!chunks.is_empty());
        let symbols: Vec<String> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(
            symbols.iter().any(|s| s.contains("fetch")),
            "should find async def fetch, got {:?}",
            symbols
        );
    }

    #[test]
    fn test_chunk_python_line_range_is_1indexed() {
        let src = "def foo():\n    return 1\n";
        let chunks = chunk_python("test.py", src, 1000);
        // Only item chunk (no prelude since the file starts with def)
        let foo = chunks
            .iter()
            .find(|c| {
                c.symbol
                    .as_deref()
                    .map(|s| s.contains("foo"))
                    .unwrap_or(false)
            })
            .expect("should find foo");
        let (start, _end) = foo.line_range.unwrap();
        assert_eq!(start, 1, "line_range should be 1-indexed");
    }
}
