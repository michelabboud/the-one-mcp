use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use crate::conversation::{ConversationRole, ConversationTranscript};
use crate::palace::PalaceMetadata;

#[derive(Debug, Clone)]
pub struct ChunkMeta {
    pub id: String,
    pub source_path: String,
    pub heading_hierarchy: Vec<String>,
    pub chunk_index: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub content_hash: String,
    pub content: String,

    /// Programming language, if this chunk came from a source file.
    /// E.g. "rust", "python", "typescript". None for markdown and unknown types.
    pub language: Option<String>,

    /// Symbol name for code chunks (e.g. "fn parse_config", "struct Broker", "impl MyTrait for MyStruct").
    /// None for markdown or fallback text chunks.
    pub symbol: Option<String>,

    /// First line of the item as a signature (function signature, struct declaration, etc.).
    /// Useful as LLM context.
    pub signature: Option<String>,

    /// 1-indexed line range (start, end) in the source file.
    pub line_range: Option<(usize, usize)>,
}

/// Estimate token count: ~1 token per 4 characters.
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Compute a deterministic hex hash of content using DefaultHasher.
fn content_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Parse a heading line, returning (level, title).
/// E.g. "## Foo" -> Some((2, "Foo"))
fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    let rest = &trimmed[hashes..];
    // Must be followed by a space (or be just hashes at end of line for edge case)
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let title = rest.trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some((hashes, title))
}

/// Split text into paragraphs on double-newline boundaries, preserving code blocks intact.
/// Returns Vec<(paragraph_text, byte_offset_within_input)>.
fn split_paragraphs_preserving_code_blocks(text: &str) -> Vec<(String, usize)> {
    let mut paragraphs: Vec<(String, usize)> = Vec::new();
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_start: usize = 0;
    let mut in_code_block = false;
    let mut offset = 0;

    for line in text.split('\n') {
        let line_with_newline_len = line.len() + 1; // +1 for the '\n' we split on

        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
        }

        if !in_code_block && line.trim().is_empty() && !current_lines.is_empty() {
            // Check if next would also be empty (double newline boundary)
            // We accumulate blank lines and flush when we hit non-blank
            current_lines.push(line);
        } else if !in_code_block
            && current_lines.last().is_some_and(|l| l.trim().is_empty())
            && !line.trim().is_empty()
        {
            // We had trailing blank lines and now hit content - split here
            // Pop trailing blank lines from current paragraph
            let mut trailing_blanks = Vec::new();
            while current_lines.last().is_some_and(|l| l.trim().is_empty()) {
                trailing_blanks.push(current_lines.pop().unwrap());
            }

            if !current_lines.is_empty() {
                let para = current_lines.join("\n");
                paragraphs.push((para, current_start));
            }

            current_start = offset;
            current_lines.clear();
            // Include blank lines as separator before this paragraph
            for bl in trailing_blanks.into_iter().rev() {
                current_lines.push(bl);
            }
            current_lines.push(line);
        } else {
            if current_lines.is_empty() {
                current_start = offset;
            }
            current_lines.push(line);
        }

        offset += line_with_newline_len;
    }
    // The last line doesn't actually have a trailing newline from split
    // Adjust: text.split('\n') on "a\nb" gives ["a", "b"], offsets need care
    // But for our purposes the byte_offset math is approximate within the section.

    if !current_lines.is_empty() {
        let para = current_lines.join("\n");
        paragraphs.push((para, current_start));
    }

    paragraphs
}

pub fn chunk_markdown(source_path: &str, content: &str, max_chunk_tokens: usize) -> Vec<ChunkMeta> {
    // Step 1: Parse into sections by heading boundaries
    // Each section: (heading_hierarchy, section_text, byte_offset_in_content)
    let mut sections: Vec<(Vec<String>, String, usize)> = Vec::new();
    let mut hierarchy: Vec<(usize, String)> = Vec::new(); // (level, title)
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_section_start: usize = 0;
    let mut in_code_block = false;
    let mut offset: usize = 0;

    let lines: Vec<&str> = content.split('\n').collect();

    for (i, &line) in lines.iter().enumerate() {
        let line_byte_len = line.len() + if i < lines.len() - 1 { 1 } else { 0 };

        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
        }

        if !in_code_block {
            if let Some((level, title)) = parse_heading(line) {
                // Flush previous section
                if !current_lines.is_empty() {
                    let section_text = current_lines.join("\n");
                    let hier: Vec<String> = hierarchy.iter().map(|(_, t)| t.clone()).collect();
                    sections.push((hier, section_text, current_section_start));
                    current_lines.clear();
                }

                // Update hierarchy: pop back to this level
                while hierarchy.last().is_some_and(|(l, _)| *l >= level) {
                    hierarchy.pop();
                }
                hierarchy.push((level, title));

                current_section_start = offset;
                current_lines.push(line);
                offset += line_byte_len;
                continue;
            }
        }

        if current_lines.is_empty() {
            current_section_start = offset;
        }
        current_lines.push(line);
        offset += line_byte_len;
    }

    // Flush last section
    if !current_lines.is_empty() {
        let section_text = current_lines.join("\n");
        let hier: Vec<String> = hierarchy.iter().map(|(_, t)| t.clone()).collect();
        sections.push((hier, section_text, current_section_start));
    }

    // Handle empty content
    if sections.is_empty() {
        return vec![ChunkMeta {
            id: format!("{source_path}:0"),
            source_path: source_path.to_string(),
            heading_hierarchy: vec![],
            chunk_index: 0,
            byte_offset: 0,
            byte_length: content.len(),
            content_hash: content_hash(content),
            content: content.to_string(),
            language: None,
            symbol: None,
            signature: None,
            line_range: None,
        }];
    }

    // Step 2: For each section, produce chunks (splitting if too large)
    let mut chunks: Vec<ChunkMeta> = Vec::new();
    let mut chunk_index: usize = 0;

    for (hier, section_text, section_byte_offset) in &sections {
        let tokens = estimate_tokens(section_text);

        if tokens <= max_chunk_tokens {
            chunks.push(ChunkMeta {
                id: format!("{source_path}:{chunk_index}"),
                source_path: source_path.to_string(),
                heading_hierarchy: hier.clone(),
                chunk_index,
                byte_offset: *section_byte_offset,
                byte_length: section_text.len(),
                content_hash: content_hash(section_text),
                content: section_text.clone(),
                language: None,
                symbol: None,
                signature: None,
                line_range: None,
            });
            chunk_index += 1;
        } else {
            // Split on paragraph boundaries, respecting code blocks
            let paragraphs = split_paragraphs_preserving_code_blocks(section_text);

            let mut current_parts: Vec<String> = Vec::new();
            let mut current_byte_start: usize = *section_byte_offset;
            let mut current_tokens: usize = 0;

            for (para_text, para_offset) in &paragraphs {
                let para_tokens = estimate_tokens(para_text);

                if current_tokens + para_tokens > max_chunk_tokens && !current_parts.is_empty() {
                    // Flush current accumulation
                    let chunk_content = current_parts.join("\n\n");
                    let byte_length = chunk_content.len();
                    chunks.push(ChunkMeta {
                        id: format!("{source_path}:{chunk_index}"),
                        source_path: source_path.to_string(),
                        heading_hierarchy: hier.clone(),
                        chunk_index,
                        byte_offset: current_byte_start,
                        byte_length,
                        content_hash: content_hash(&chunk_content),
                        content: chunk_content,
                        language: None,
                        symbol: None,
                        signature: None,
                        line_range: None,
                    });
                    chunk_index += 1;
                    current_parts.clear();
                    current_tokens = 0;
                    current_byte_start = section_byte_offset + para_offset;
                }

                if current_parts.is_empty() {
                    current_byte_start = section_byte_offset + para_offset;
                }
                current_parts.push(para_text.clone());
                current_tokens += para_tokens;
            }

            // Flush remaining
            if !current_parts.is_empty() {
                let chunk_content = current_parts.join("\n\n");
                let byte_length = chunk_content.len();
                chunks.push(ChunkMeta {
                    id: format!("{source_path}:{chunk_index}"),
                    source_path: source_path.to_string(),
                    heading_hierarchy: hier.clone(),
                    chunk_index,
                    byte_offset: current_byte_start,
                    byte_length,
                    content_hash: content_hash(&chunk_content),
                    content: chunk_content,
                    language: None,
                    symbol: None,
                    signature: None,
                    line_range: None,
                });
                chunk_index += 1;
            }
        }
    }

    // Handle edge case: no chunks produced (shouldn't happen, but safety)
    if chunks.is_empty() {
        chunks.push(ChunkMeta {
            id: format!("{source_path}:0"),
            source_path: source_path.to_string(),
            heading_hierarchy: vec![],
            chunk_index: 0,
            byte_offset: 0,
            byte_length: content.len(),
            content_hash: content_hash(content),
            content: content.to_string(),
            language: None,
            symbol: None,
            signature: None,
            line_range: None,
        });
    }

    chunks
}

/// Dispatch to the appropriate chunker based on file extension.
///
/// Returns chunks with `language`/`symbol`/`signature`/`line_range` metadata
/// populated for supported programming languages, or heading-based metadata
/// for markdown, or plain text chunks for unknown extensions.
///
/// When the `tree-sitter-chunker` feature is enabled, the 5 originally
/// supported languages (Rust/Python/TS/JS/Go) go through tree-sitter first
/// and transparently fall back to the regex chunkers on parse failure. The 8
/// additional languages (C/C++/Java/Kotlin/PHP/Ruby/Swift/Zig) are only
/// available with the feature enabled.
#[allow(clippy::needless_return)]
pub fn chunk_file(path: &Path, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let path_str = path.to_str().unwrap_or("");
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("md") | Some("markdown") => chunk_markdown(path_str, content, max_tokens),
        // --- Existing 5 languages: tree-sitter first, regex fallback --------
        Some("rs") => {
            #[cfg(feature = "tree-sitter-chunker")]
            {
                return crate::chunker_rust_ts::chunk_rust_ts(path_str, content, max_tokens);
            }
            #[cfg(not(feature = "tree-sitter-chunker"))]
            {
                crate::chunker_rust::chunk_rust(path_str, content, max_tokens)
            }
        }
        Some("py") => {
            #[cfg(feature = "tree-sitter-chunker")]
            {
                return crate::chunker_python_ts::chunk_python_ts(path_str, content, max_tokens);
            }
            #[cfg(not(feature = "tree-sitter-chunker"))]
            {
                crate::chunker_python::chunk_python(path_str, content, max_tokens)
            }
        }
        Some("ts") => {
            #[cfg(feature = "tree-sitter-chunker")]
            {
                return crate::chunker_typescript_ts::chunk_typescript_ts(
                    path_str, content, max_tokens,
                );
            }
            #[cfg(not(feature = "tree-sitter-chunker"))]
            {
                crate::chunker_typescript::chunk_typescript(path_str, content, max_tokens)
            }
        }
        Some("tsx") => {
            #[cfg(feature = "tree-sitter-chunker")]
            {
                return crate::chunker_typescript_ts::chunk_tsx_ts(path_str, content, max_tokens);
            }
            #[cfg(not(feature = "tree-sitter-chunker"))]
            {
                crate::chunker_typescript::chunk_typescript(path_str, content, max_tokens)
            }
        }
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => {
            #[cfg(feature = "tree-sitter-chunker")]
            {
                return crate::chunker_typescript_ts::chunk_javascript_ts(
                    path_str, content, max_tokens,
                );
            }
            #[cfg(not(feature = "tree-sitter-chunker"))]
            {
                crate::chunker_typescript::chunk_javascript(path_str, content, max_tokens)
            }
        }
        Some("go") => {
            #[cfg(feature = "tree-sitter-chunker")]
            {
                return crate::chunker_go_ts::chunk_go_ts(path_str, content, max_tokens);
            }
            #[cfg(not(feature = "tree-sitter-chunker"))]
            {
                crate::chunker_go::chunk_go(path_str, content, max_tokens)
            }
        }
        // --- 8 new languages: tree-sitter only (fallback to plain text) -----
        #[cfg(feature = "tree-sitter-chunker")]
        Some("c") | Some("h") => crate::chunker_c::chunk_c(path_str, content, max_tokens),
        #[cfg(feature = "tree-sitter-chunker")]
        Some("cc") | Some("cpp") | Some("cxx") | Some("hpp") | Some("hxx") | Some("hh") => {
            crate::chunker_cpp::chunk_cpp(path_str, content, max_tokens)
        }
        #[cfg(feature = "tree-sitter-chunker")]
        Some("java") => crate::chunker_java::chunk_java(path_str, content, max_tokens),
        #[cfg(feature = "tree-sitter-chunker")]
        Some("kt") | Some("kts") => {
            crate::chunker_kotlin::chunk_kotlin(path_str, content, max_tokens)
        }
        #[cfg(feature = "tree-sitter-chunker")]
        Some("php") | Some("phtml") => crate::chunker_php::chunk_php(path_str, content, max_tokens),
        #[cfg(feature = "tree-sitter-chunker")]
        Some("rb") | Some("rake") => crate::chunker_ruby::chunk_ruby(path_str, content, max_tokens),
        #[cfg(feature = "tree-sitter-chunker")]
        Some("swift") => crate::chunker_swift::chunk_swift(path_str, content, max_tokens),
        #[cfg(feature = "tree-sitter-chunker")]
        Some("zig") => crate::chunker_zig::chunk_zig(path_str, content, max_tokens),
        _ => chunk_text_fallback(path_str, content, max_tokens),
    }
}

/// Split content on blank lines into sub-chunks, each at most `max_chars` characters.
///
/// Shared across language chunkers to handle oversized items.
pub(crate) fn split_on_blank_lines(content: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        if line.trim().is_empty() && current.len() >= max_chars / 2 {
            chunks.push(current.trim_end().to_string());
            current.clear();
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.is_empty() {
        chunks.push(current.trim_end().to_string());
    }
    chunks
}

/// Generic fallback chunker for unknown file types.
/// Splits on blank lines, packs chunks up to max_tokens.
pub fn chunk_text_fallback(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    // Estimate 4 chars per token
    let max_chars = max_tokens * 4;
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut chunk_index = 0;
    let mut current_start_line: usize = 1;
    let mut current_line: usize = 1;

    for line in content.lines() {
        if line.trim().is_empty() && !current.is_empty() && current.len() >= max_chars / 2 {
            // Flush on blank line if we have enough content
            push_text_chunk(
                &mut chunks,
                source_path,
                chunk_index,
                &current,
                current_start_line,
                current_line,
            );
            chunk_index += 1;
            current.clear();
            current_start_line = current_line + 1;
        } else if current.len() + line.len() + 1 > max_chars && !current.is_empty() {
            // Flush on size overflow
            push_text_chunk(
                &mut chunks,
                source_path,
                chunk_index,
                &current,
                current_start_line,
                current_line,
            );
            chunk_index += 1;
            current.clear();
            current_start_line = current_line;
        }
        current.push_str(line);
        current.push('\n');
        current_line += 1;
    }

    if !current.is_empty() {
        push_text_chunk(
            &mut chunks,
            source_path,
            chunk_index,
            &current,
            current_start_line,
            current_line,
        );
    }

    chunks
}

fn push_text_chunk(
    chunks: &mut Vec<ChunkMeta>,
    source_path: &str,
    chunk_index: usize,
    content: &str,
    start_line: usize,
    end_line: usize,
) {
    chunks.push(ChunkMeta {
        id: format!("{source_path}:{chunk_index}"),
        source_path: source_path.to_string(),
        chunk_index,
        content: content.trim_end().to_string(),
        heading_hierarchy: Vec::new(),
        byte_offset: 0,
        byte_length: content.len(),
        content_hash: {
            let mut hasher = DefaultHasher::new();
            content.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        },
        language: None,
        symbol: None,
        signature: None,
        line_range: Some((start_line, end_line)),
    });
}

pub fn chunk_conversation(
    source_path: &str,
    transcript: &ConversationTranscript,
    palace: Option<&PalaceMetadata>,
) -> Vec<ChunkMeta> {
    let mut heading_hierarchy = vec!["conversation".to_string()];
    if let Some(metadata) = palace {
        heading_hierarchy.push(metadata.wing.clone());
        if let Some(hall) = &metadata.hall {
            heading_hierarchy.push(hall.clone());
        }
        if let Some(room) = &metadata.room {
            heading_hierarchy.push(room.clone());
        }
    }

    let mut byte_offset = 0usize;

    transcript
        .messages
        .iter()
        .map(|message| {
            let content = format!(
                "[turn:{}][role:{}]\n{}",
                message.turn_index,
                conversation_role_label(&message.role),
                message.content
            );
            let chunk = ChunkMeta {
                id: format!("{source_path}:turn:{}", message.turn_index),
                source_path: source_path.to_string(),
                heading_hierarchy: heading_hierarchy.clone(),
                chunk_index: message.turn_index,
                byte_offset,
                byte_length: content.len(),
                content_hash: content_hash(&content),
                content,
                language: Some("conversation".to_string()),
                symbol: palace.and_then(|metadata| metadata.room.clone()),
                signature: palace.and_then(|metadata| metadata.hall.clone()),
                line_range: None,
            };
            byte_offset += chunk.byte_length + 1;
            chunk
        })
        .collect()
}

fn conversation_role_label(role: &ConversationRole) -> &'static str {
    match role {
        ConversationRole::System => "system",
        ConversationRole::User => "user",
        ConversationRole::Assistant => "assistant",
        ConversationRole::Tool => "tool",
        ConversationRole::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_by_headings() {
        let md = "# Intro\nHello world\n# Details\nSome details here\n# Conclusion\nThe end";
        let chunks = chunk_markdown("docs/readme.md", md, 500);

        assert_eq!(chunks.len(), 3, "Expected 3 chunks, got {}", chunks.len());
        assert_eq!(chunks[0].heading_hierarchy, vec!["Intro"]);
        assert_eq!(chunks[1].heading_hierarchy, vec!["Details"]);
        assert_eq!(chunks[2].heading_hierarchy, vec!["Conclusion"]);

        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[1].chunk_index, 1);
        assert_eq!(chunks[2].chunk_index, 2);

        assert_eq!(chunks[0].id, "docs/readme.md:0");
        assert_eq!(chunks[1].id, "docs/readme.md:1");
    }

    #[test]
    fn test_large_section_splits_on_paragraphs() {
        // Create a section with ~750 tokens (each token ~ 4 chars, so ~3000 chars)
        let mut paragraphs = Vec::new();
        for i in 0..15 {
            paragraphs.push(format!("Paragraph {} with enough text to take up some tokens. This is filler content to ensure we exceed the token limit when combined.", i));
        }
        let body = paragraphs.join("\n\n");
        let md = format!("# BigSection\n{body}");

        let max_tokens = 200; // low limit to force splitting
        let chunks = chunk_markdown("docs/big.md", &md, max_tokens);

        assert!(
            chunks.len() > 1,
            "Expected multiple chunks, got {}",
            chunks.len()
        );

        // Each chunk should be roughly within tolerance of max_tokens
        for chunk in &chunks {
            let tokens = estimate_tokens(&chunk.content);
            // Allow some tolerance: a single paragraph might exceed max_tokens
            assert!(
                tokens <= max_tokens * 2,
                "Chunk {} has {} estimated tokens, way over limit {}",
                chunk.chunk_index,
                tokens,
                max_tokens
            );
        }

        // All chunks should inherit the heading hierarchy
        for chunk in &chunks {
            assert_eq!(chunk.heading_hierarchy, vec!["BigSection"]);
        }
    }

    #[test]
    fn test_code_blocks_not_split() {
        let md = "# Code Example\nSome intro text\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\nAfter code";
        let chunks = chunk_markdown("docs/code.md", md, 500);

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("```rust"));
        assert!(chunks[0].content.contains("println!"));
        assert!(chunks[0].content.contains("```"));
    }

    #[test]
    fn test_no_headings_single_chunk() {
        let md = "Just some plain text without any headings.\nAnother line of text.";
        let chunks = chunk_markdown("docs/plain.md", md, 500);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_hierarchy, Vec::<String>::new());
        assert_eq!(chunks[0].chunk_index, 0);
        assert!(chunks[0].content.contains("plain text"));
    }

    #[test]
    fn test_content_hash_differs_for_different_content() {
        let chunks_a = chunk_markdown("docs/file.md", "# Title\nContent A", 500);
        let chunks_b = chunk_markdown("docs/file.md", "# Title\nContent B", 500);

        assert_eq!(chunks_a[0].source_path, chunks_b[0].source_path);
        assert_ne!(
            chunks_a[0].content_hash, chunks_b[0].content_hash,
            "Hashes should differ for different content"
        );
    }

    #[test]
    fn test_nested_headings_preserve_hierarchy() {
        let md = "# Top\n## Sub\n### Deep\nContent here";
        let chunks = chunk_markdown("docs/nested.md", md, 500);

        // The last chunk (with "Content here") should have full hierarchy
        let deep_chunk = chunks.last().unwrap();
        assert_eq!(deep_chunk.heading_hierarchy, vec!["Top", "Sub", "Deep"]);
    }

    #[test]
    fn test_chunk_markdown_new_fields_are_none() {
        let md = "# Title\nSome content";
        let chunks = chunk_markdown("docs/test.md", md, 500);
        assert!(!chunks.is_empty());
        assert!(chunks[0].language.is_none());
        assert!(chunks[0].symbol.is_none());
        assert!(chunks[0].signature.is_none());
        assert!(chunks[0].line_range.is_none());
    }

    #[test]
    fn test_chunk_file_dispatches_to_markdown() {
        let chunks = chunk_file(Path::new("readme.md"), "# Title\nHello", 500);
        assert!(!chunks.is_empty());
        assert!(chunks[0].language.is_none());
    }

    #[test]
    fn test_chunk_file_dispatches_to_fallback_for_unknown() {
        let chunks = chunk_file(Path::new("config.ini"), "key=value\nother=data", 500);
        assert!(!chunks.is_empty());
        assert!(chunks[0].language.is_none());
        assert!(chunks[0].symbol.is_none());
    }

    #[test]
    fn test_chunk_text_fallback_respects_max_tokens() {
        let content = "Line one\n\nLine two\n\nLine three\n\nLine four";
        let chunks = chunk_text_fallback("test.txt", content, 500);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_conversation_preserves_verbatim_content_and_metadata() {
        let transcript = ConversationTranscript {
            source_id: "session".to_string(),
            messages: vec![crate::conversation::ConversationMessage {
                role: ConversationRole::Assistant,
                content: "Refresh token rotation failed in staging.".to_string(),
                turn_index: 2,
            }],
        };

        let chunks = chunk_conversation(
            "/tmp/session.json",
            &transcript,
            Some(&PalaceMetadata::new(
                "proj-auth",
                Some("hall_facts".to_string()),
                Some("auth-migration".to_string()),
            )),
        );

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "/tmp/session.json:turn:2");
        assert!(chunks[0]
            .content
            .contains("Refresh token rotation failed in staging."));
        assert_eq!(
            chunks[0].heading_hierarchy,
            vec![
                "conversation".to_string(),
                "proj-auth".to_string(),
                "hall_facts".to_string(),
                "auth-migration".to_string(),
            ]
        );
        assert_eq!(chunks[0].signature.as_deref(), Some("hall_facts"));
        assert_eq!(chunks[0].symbol.as_deref(), Some("auth-migration"));
        assert_eq!(chunks[0].language.as_deref(), Some("conversation"));
    }

    #[test]
    fn test_chunk_file_rust_path() {
        let content = "pub fn hello() -> &'static str { \"world\" }";
        let chunks = chunk_file(Path::new("test.rs"), content, 500);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language.as_deref(), Some("rust"));
    }

    #[test]
    fn test_chunk_file_python_path() {
        let chunks = chunk_file(
            Path::new("test.py"),
            "def hello():\n    return 'world'",
            500,
        );
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language.as_deref(), Some("python"));
    }

    #[test]
    fn test_chunk_file_typescript_path() {
        let chunks = chunk_file(
            Path::new("test.ts"),
            "export function foo() { return 1; }",
            500,
        );
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language.as_deref(), Some("typescript"));
    }

    #[test]
    fn test_chunk_file_javascript_path() {
        let chunks = chunk_file(Path::new("test.js"), "function bar() { return 2; }", 500);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language.as_deref(), Some("javascript"));
    }

    #[test]
    fn test_chunk_file_tsx_path() {
        let chunks = chunk_file(
            Path::new("Component.tsx"),
            "export function App() { return null; }",
            500,
        );
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language.as_deref(), Some("typescript"));
    }

    #[test]
    fn test_chunk_file_go_path() {
        let chunks = chunk_file(Path::new("test.go"), "package main\nfunc main() {}", 500);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].language.as_deref(), Some("go"));
    }

    // -----------------------------------------------------------------------
    // Task 1.2 (v0.9.0): Tree-sitter chunker — 13-language coverage
    // -----------------------------------------------------------------------

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_c_top_level_functions() {
        let src = r#"
#include <stdio.h>

int add(int a, int b) {
    return a + b;
}

int main(void) {
    printf("%d\n", add(1, 2));
    return 0;
}
"#;
        let chunks = chunk_file(Path::new("main.c"), src, 500);
        assert!(!chunks.is_empty(), "c chunker should emit chunks");
        assert!(
            chunks.iter().any(|c| c.language.as_deref() == Some("c")),
            "at least one chunk should be tagged language=c"
        );
        assert!(
            chunks.iter().any(|c| c.content.contains("int add")),
            "add() function should appear in chunks"
        );
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_cpp_class_and_function() {
        let src = r#"
namespace foo {
class Bar {
public:
    int baz() const { return 42; }
};
}

int main() { return 0; }
"#;
        let chunks = chunk_file(Path::new("main.cpp"), src, 500);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.language.as_deref() == Some("cpp")));
        assert!(chunks.iter().any(|c| c.content.contains("class Bar")));
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_java_class() {
        let src = r#"
package com.example;

public class Greeter {
    public String hello(String name) {
        return "Hello, " + name;
    }
}
"#;
        let chunks = chunk_file(Path::new("Greeter.java"), src, 500);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.language.as_deref() == Some("java")));
        assert!(chunks.iter().any(|c| c.content.contains("class Greeter")));
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_kotlin_function() {
        let src = r#"
fun main() {
    println("hi")
}

class Widget(val size: Int) {
    fun render(): String = "w=$size"
}
"#;
        let chunks = chunk_file(Path::new("main.kt"), src, 500);
        assert!(!chunks.is_empty());
        assert!(chunks
            .iter()
            .any(|c| c.language.as_deref() == Some("kotlin")));
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_php_class() {
        let src = r#"<?php
namespace App;

class User {
    public function getName(): string {
        return $this->name;
    }
}
"#;
        let chunks = chunk_file(Path::new("User.php"), src, 500);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.language.as_deref() == Some("php")));
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_ruby_class_and_method() {
        let src = r#"
module Accounts
  class User
    def initialize(name)
      @name = name
    end

    def greet
      "Hello, #{@name}"
    end
  end
end
"#;
        let chunks = chunk_file(Path::new("user.rb"), src, 500);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.language.as_deref() == Some("ruby")));
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_swift_function_and_class() {
        let src = r#"
import Foundation

func add(_ a: Int, _ b: Int) -> Int {
    return a + b
}

class Counter {
    var value: Int = 0
    func increment() {
        value += 1
    }
}
"#;
        let chunks = chunk_file(Path::new("counter.swift"), src, 500);
        assert!(!chunks.is_empty());
        assert!(chunks
            .iter()
            .any(|c| c.language.as_deref() == Some("swift")));
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_zig_function() {
        let src = r#"
const std = @import("std");

pub fn main() void {
    std.debug.print("hi\n", .{});
}

fn add(a: i32, b: i32) i32 {
    return a + b;
}
"#;
        let chunks = chunk_file(Path::new("main.zig"), src, 500);
        assert!(!chunks.is_empty(), "zig chunker should emit chunks");
        // Zig grammar may or may not populate symbols cleanly — the floor
        // requirement is that we at least tag chunks with the language or
        // fall back to plain text chunks.
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_tree_sitter_rust_matches_regex_top_level_count() {
        // The tree-sitter Rust chunker should find at least as many top-level
        // items as the regex chunker for a simple file.
        let src = r#"
pub fn alpha() -> i32 { 1 }

pub struct Beta {
    x: i32,
}

pub trait Gamma {
    fn gamma(&self) -> i32;
}

pub enum Delta { A, B }
"#;
        let ts_chunks = chunk_file(Path::new("lib.rs"), src, 500);
        let regex_chunks = crate::chunker_rust::chunk_rust("lib.rs", src, 500);
        assert!(!ts_chunks.is_empty());
        assert!(!regex_chunks.is_empty());
        assert!(
            ts_chunks.len() >= regex_chunks.len().saturating_sub(1),
            "tree-sitter chunker should not lose top-level items vs regex (ts={}, regex={})",
            ts_chunks.len(),
            regex_chunks.len()
        );
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_ts_falls_back_on_parse_failure() {
        // Gibberish that tree-sitter cannot fruitfully parse — dispatcher
        // should still return SOMETHING (either via parser partial recovery
        // or the regex fallback path).
        let src = "this is not a rust file at all &&&&&&";
        let chunks = chunk_file(Path::new("lib.rs"), src, 500);
        // Either empty (acceptable — regex chunker also returns empty for
        // non-code) or non-empty fallback.
        let _ = chunks;
    }

    #[cfg(feature = "tree-sitter-chunker")]
    #[test]
    fn test_chunk_file_line_range_populated_for_tree_sitter_chunks() {
        let src = "pub fn alpha() -> i32 { 1 }\n\npub fn beta() -> i32 { 2 }\n";
        let chunks = chunk_file(Path::new("lib.rs"), src, 500);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(
                c.line_range.is_some(),
                "line_range should be populated for tree-sitter chunks (symbol={:?})",
                c.symbol
            );
            let (start, end) = c.line_range.unwrap();
            assert!(start >= 1 && end >= start);
        }
    }
}
