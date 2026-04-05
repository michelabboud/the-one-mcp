//! Tree-sitter–based PHP chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TOP_LEVEL_KINDS: &[&str] = &[
    "function_definition",
    "class_declaration",
    "interface_declaration",
    "trait_declaration",
    "enum_declaration",
    "namespace_definition",
    "use_declaration",
];

pub fn chunk_php(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "php",
        source_path,
        content,
        max_tokens,
        TOP_LEVEL_KINDS,
        "name",
    );
    if chunks.is_empty() {
        return crate::chunker::chunk_text_fallback(source_path, content, max_tokens);
    }
    chunks
}
