//! Tree-sitter–based Python chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TOP_LEVEL_KINDS: &[&str] = &[
    "function_definition",
    "class_definition",
    "decorated_definition",
    "async_function_definition",
];

pub fn chunk_python_ts(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "python",
        source_path,
        content,
        max_tokens,
        TOP_LEVEL_KINDS,
        "name",
    );
    if chunks.is_empty() {
        return crate::chunker_python::chunk_python(source_path, content, max_tokens);
    }
    chunks
}
