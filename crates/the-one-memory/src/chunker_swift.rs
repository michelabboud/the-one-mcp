//! Tree-sitter–based Swift chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TOP_LEVEL_KINDS: &[&str] = &[
    "function_declaration",
    "class_declaration",
    "protocol_declaration",
    "enum_declaration",
    "extension_declaration",
    "property_declaration",
    "import_declaration",
    "typealias_declaration",
];

pub fn chunk_swift(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_swift::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "swift",
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
