//! Tree-sitter–based Rust chunker.
//!
//! Replaces [`crate::chunker_rust::chunk_rust`] when the
//! `tree-sitter-chunker` feature is enabled. Falls back to the regex chunker
//! on parse failure.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TOP_LEVEL_KINDS: &[&str] = &[
    "function_item",
    "impl_item",
    "trait_item",
    "struct_item",
    "enum_item",
    "union_item",
    "mod_item",
    "type_item",
    "const_item",
    "static_item",
    "macro_definition",
    "foreign_mod_item",
];

pub fn chunk_rust_ts(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "rust",
        source_path,
        content,
        max_tokens,
        TOP_LEVEL_KINDS,
        "name",
    );
    if chunks.is_empty() {
        // Fall back to regex chunker on parse failure or empty-top-level file
        return crate::chunker_rust::chunk_rust(source_path, content, max_tokens);
    }
    chunks
}
