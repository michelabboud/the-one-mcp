//! Tree-sitter–based C chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TOP_LEVEL_KINDS: &[&str] = &[
    "function_definition",
    "declaration",
    "struct_specifier",
    "enum_specifier",
    "union_specifier",
    "type_definition",
    "preproc_def",
    "preproc_function_def",
    "preproc_include",
    "preproc_ifdef",
];

pub fn chunk_c(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_c::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "c",
        source_path,
        content,
        max_tokens,
        TOP_LEVEL_KINDS,
        "declarator",
    );
    if chunks.is_empty() {
        return crate::chunker::chunk_text_fallback(source_path, content, max_tokens);
    }
    chunks
}
