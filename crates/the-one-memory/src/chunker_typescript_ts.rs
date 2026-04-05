//! Tree-sitter–based TypeScript / JavaScript chunker.

#![cfg(feature = "tree-sitter-chunker")]

use crate::chunker::ChunkMeta;

const TS_TOP_LEVEL_KINDS: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "class_declaration",
    "interface_declaration",
    "type_alias_declaration",
    "enum_declaration",
    "lexical_declaration",
    "variable_statement",
    "export_statement",
    "abstract_class_declaration",
    "internal_module",
    "module",
    "namespace_declaration",
];

const JS_TOP_LEVEL_KINDS: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "class_declaration",
    "lexical_declaration",
    "variable_declaration",
    "export_statement",
];

pub fn chunk_typescript_ts(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "typescript",
        source_path,
        content,
        max_tokens,
        TS_TOP_LEVEL_KINDS,
        "name",
    );
    if chunks.is_empty() {
        return crate::chunker_typescript::chunk_typescript(source_path, content, max_tokens);
    }
    chunks
}

pub fn chunk_tsx_ts(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TSX.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "typescript",
        source_path,
        content,
        max_tokens,
        TS_TOP_LEVEL_KINDS,
        "name",
    );
    if chunks.is_empty() {
        return crate::chunker_typescript::chunk_typescript(source_path, content, max_tokens);
    }
    chunks
}

pub fn chunk_javascript_ts(source_path: &str, content: &str, max_tokens: usize) -> Vec<ChunkMeta> {
    let language: tree_sitter::Language = tree_sitter_javascript::LANGUAGE.into();
    let chunks = crate::chunker_ts_impl::chunk_with_tree_sitter(
        &language,
        "javascript",
        source_path,
        content,
        max_tokens,
        JS_TOP_LEVEL_KINDS,
        "name",
    );
    if chunks.is_empty() {
        return crate::chunker_typescript::chunk_javascript(source_path, content, max_tokens);
    }
    chunks
}
