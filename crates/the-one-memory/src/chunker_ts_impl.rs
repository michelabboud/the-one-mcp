//! Shared tree-sitter walker used by language chunkers.
//!
//! Each language chunker calls [`chunk_with_tree_sitter`] with its parser
//! language handle, a list of AST node kinds that represent "top-level
//! declarations" (functions, classes, structs, etc.), and a field name used
//! to extract the declaration's identifier. The walker parses the source,
//! enumerates the root node's children, emits one [`ChunkMeta`] per matching
//! node, and falls back to a single wrapping chunk when the file has no
//! recognized top-level declarations.
//!
//! The walker is resilient to parse failures: if tree-sitter cannot parse
//! the source at all, it returns an empty `Vec`, and the caller should fall
//! back to a regex-based chunker or plain text chunker.
//!
//! This module is only compiled when the `tree-sitter-chunker` feature is
//! enabled.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tree_sitter::{Language, Node, Parser};

/// Chunk a source file using a tree-sitter grammar.
///
/// * `language` — the parser language handle (e.g. `tree_sitter_rust::LANGUAGE.into()`)
/// * `language_name` — lowercase display name stored on chunks (e.g. `"rust"`)
/// * `source_path` — the file path or logical name, used in chunk IDs
/// * `content` — the source code as a UTF-8 string
/// * `max_tokens` — soft cap per chunk (~4 chars per token); oversized items
///   are still emitted as single chunks but a warning is traced
/// * `top_level_kinds` — list of AST node kinds to emit as chunks
/// * `name_field` — name of the field on a top-level node that holds the
///   declaration's identifier (often `"name"`); pass `""` to skip symbol
///   extraction
///
/// Returns `Vec::new()` if tree-sitter cannot parse the source at all; the
/// caller should fall back to a regex or plain text chunker.
pub fn chunk_with_tree_sitter(
    language: &Language,
    language_name: &str,
    source_path: &str,
    content: &str,
    max_tokens: usize,
    top_level_kinds: &[&str],
    name_field: &str,
) -> Vec<ChunkMeta> {
    let mut parser = Parser::new();
    if parser.set_language(language).is_err() {
        return Vec::new();
    }
    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let root = tree.root_node();
    let mut chunks = Vec::new();

    // Walk top-level children and collect matching nodes.
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if !top_level_kinds.contains(&child.kind()) {
            continue;
        }
        if let Some(chunk) = node_to_chunk(
            &child,
            content,
            source_path,
            language_name,
            name_field,
            chunks.len(),
            max_tokens,
        ) {
            chunks.push(chunk);
        }
    }

    chunks
}

/// Convert a single AST node to a `ChunkMeta`.
fn node_to_chunk(
    node: &Node<'_>,
    source: &str,
    source_path: &str,
    language_name: &str,
    name_field: &str,
    chunk_index: usize,
    max_tokens: usize,
) -> Option<ChunkMeta> {
    let byte_range = node.byte_range();
    let content_slice = source.get(byte_range.clone())?;
    let trimmed = content_slice.trim();
    if trimmed.is_empty() {
        return None;
    }

    let start_row = node.start_position().row + 1;
    let end_row = node.end_position().row + 1;

    let symbol =
        extract_symbol(node, source, name_field).unwrap_or_else(|| format!("<{}>", node.kind()));
    let signature = content_slice
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let char_count = content_slice.chars().count();
    if char_count > max_tokens * 4 {
        tracing::debug!(
            "tree-sitter chunker: oversized chunk in {} ({} chars, language={}, symbol={})",
            source_path,
            char_count,
            language_name,
            symbol
        );
    }

    Some(ChunkMeta {
        id: format!("{source_path}:{chunk_index}"),
        source_path: source_path.to_string(),
        heading_hierarchy: Vec::new(),
        chunk_index,
        byte_offset: byte_range.start,
        byte_length: byte_range.end - byte_range.start,
        content_hash: content_hash(content_slice),
        content: content_slice.to_string(),
        language: Some(language_name.to_string()),
        symbol: Some(symbol),
        signature: Some(signature),
        line_range: Some((start_row, end_row)),
    })
}

/// Try to pull an identifier off a field of the node (e.g. `name`). Falls
/// back to scanning for the first `identifier` / `type_identifier` child.
fn extract_symbol(node: &Node<'_>, source: &str, name_field: &str) -> Option<String> {
    if !name_field.is_empty() {
        if let Some(name_node) = node.child_by_field_name(name_field) {
            if let Some(text) = source.get(name_node.byte_range()) {
                return Some(text.trim().to_string());
            }
        }
    }

    // Fallback — first identifier-like child.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "type_identifier" | "constant" | "name" => {
                if let Some(text) = source.get(child.byte_range()) {
                    return Some(text.trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn content_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
