//! Tree-sitter–based Kotlin chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TOP_LEVEL_KINDS: &[&str] = &[
    "function_declaration",
    "class_declaration",
    "object_declaration",
    "property_declaration",
    "type_alias",
    "companion_object",
];

pub fn chunk_kotlin(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "kotlin",
        source_path,
        content,
        max_tokens,
        TOP_LEVEL_KINDS,
        "",
    );
    if chunks.is_empty() {
        return crate::chunker::chunk_text_fallback(source_path, content, max_tokens);
    }
    chunks
}
