use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
        });
    }

    chunks
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
        assert_eq!(
            deep_chunk.heading_hierarchy,
            vec!["Top", "Sub", "Deep"]
        );
    }
}
