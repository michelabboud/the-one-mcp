//! Tree-sitter–based Zig chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

// Zig's tree-sitter grammar uses PascalCase node names since it mirrors the
// reference Zig grammar. Common top-level declaration kinds:
const TOP_LEVEL_KINDS: &[&str] = &[
    "FnProto",
    "VarDecl",
    "ContainerDecl",
    "Decl",
    "TopLevelDecl",
    "TopLevelFn",
    "TopLevelVar",
    "TopLevelComptime",
    "function_declaration",
    "variable_declaration",
];

pub fn chunk_zig(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_zig::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "zig",
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
