//! Tree-sitter–based Ruby chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TOP_LEVEL_KINDS: &[&str] = &[
    "method",
    "singleton_method",
    "class",
    "module",
    "singleton_class",
];

pub fn chunk_ruby(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_ruby::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "ruby",
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
