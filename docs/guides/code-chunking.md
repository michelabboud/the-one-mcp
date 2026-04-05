# Code-Aware Chunking Guide

> v0.9.0 — tree-sitter AST chunking for 13 programming languages, with regex fallback for the original 5.

## Why Language-Aware Chunking Matters

When you index a source file as plain text, the chunker has no concept of code structure. A large function might be split mid-body, or an entire file might be collapsed into a single blob. Either way, the chunk boundaries don't match how developers think about code — and so retrieval quality suffers.

Language-aware chunking solves this by respecting the syntactic boundaries that matter:

- **Better retrieval quality** — a `memory.search` query for "how does the parser handle errors" returns the `parse_error` function as a complete, coherent chunk, not a fragment that starts three lines into the function body.
- **Symbol metadata** — search results now include the function/struct/class name, so the LLM can immediately tell what it's looking at without reading the full text.
- **Signature context** — the first line of each item (the function signature, struct declaration, etc.) is surfaced as a separate `signature` field, giving the LLM fast access to types and parameters.
- **Line ranges** — each chunk carries the `(start, end)` line numbers from the source file. The LLM can cite exact locations: "see `auth.rs` lines 42–89".
- **Smaller, focused prompts** — when the LLM retrieves a function chunk, it gets the whole function and nothing else. No surrounding boilerplate, no adjacent unrelated code.

---

## Supported Languages

As of v0.9.0, the chunker uses the [tree-sitter](https://tree-sitter.github.io/tree-sitter/)
incremental parser with community-maintained grammars. Tree-sitter walks the real AST so
edge cases that regex would miss (generics, nested templates, complex where-clauses) are
handled correctly.

### Tier 1 — tree-sitter with regex fallback (original 5)

| Language | File Extensions | AST Node Kinds |
|----------|----------------|----------------|
| Rust | `.rs` | `function_item`, `impl_item`, `trait_item`, `struct_item`, `enum_item`, `union_item`, `mod_item`, `type_item`, `const_item`, `static_item`, `macro_definition`, `foreign_mod_item` |
| Python | `.py` | `function_definition`, `async_function_definition`, `class_definition`, `decorated_definition` |
| TypeScript | `.ts`, `.tsx` | `function_declaration`, `class_declaration`, `interface_declaration`, `type_alias_declaration`, `enum_declaration`, `lexical_declaration`, `export_statement`, `namespace_declaration` |
| JavaScript | `.js`, `.jsx`, `.mjs`, `.cjs` | `function_declaration`, `class_declaration`, `lexical_declaration`, `variable_declaration`, `export_statement` |
| Go | `.go` | `function_declaration`, `method_declaration`, `type_declaration`, `var_declaration`, `const_declaration`, `import_declaration` |

If tree-sitter fails to parse the file (malformed source, partial file, etc.), the dispatcher
transparently falls back to the v0.8.0 regex-based chunker for these five languages — users
get no regression, even on pathological input.

### Tier 2 — tree-sitter only (8 new languages in v0.9.0)

| Language | File Extensions | Key Node Kinds |
|----------|----------------|----------------|
| C | `.c`, `.h` | `function_definition`, `struct_specifier`, `enum_specifier`, `union_specifier`, `type_definition`, `preproc_*` |
| C++ | `.cc`, `.cpp`, `.cxx`, `.hpp`, `.hxx`, `.hh` | `function_definition`, `class_specifier`, `namespace_definition`, `template_declaration`, `alias_declaration` |
| Java | `.java` | `class_declaration`, `interface_declaration`, `enum_declaration`, `record_declaration`, `method_declaration`, `constructor_declaration` |
| Kotlin | `.kt`, `.kts` | `function_declaration`, `class_declaration`, `object_declaration`, `property_declaration`, `type_alias` |
| PHP | `.php`, `.phtml` | `function_definition`, `class_declaration`, `interface_declaration`, `trait_declaration`, `enum_declaration`, `namespace_definition` |
| Ruby | `.rb`, `.rake` | `method`, `singleton_method`, `class`, `module`, `singleton_class` |
| Swift | `.swift` | `function_declaration`, `class_declaration`, `protocol_declaration`, `enum_declaration`, `extension_declaration` |
| Zig | `.zig` | `FnProto`, `VarDecl`, `ContainerDecl`, `TopLevelDecl` |

For any other extension, the chunker falls back to blank-line paragraph chunking — completely
safe, just without symbol metadata.

### Feature flag

All tree-sitter chunkers live behind the `tree-sitter-chunker` Cargo feature (default on).
Lean builds that disable default features (e.g. Intel Mac lean) retain the original regex
chunkers for the 5 Tier 1 languages but do not get any of the 8 new languages.

```bash
# Default build — all 13 languages
cargo build --release

# Lean build — original 5 languages (regex) only
cargo build --release --no-default-features --features the-one-ui/embed-swagger
```

---

## What Each Chunker Respects

### Rust (`.rs`)

The Rust chunker uses brace-depth tracking to find complete top-level items. It recognizes:

- `fn foo(` and `async fn foo(` — free functions and async functions
- `pub fn`, `pub(crate) fn`, `pub(super) fn` — visibility prefixes ignored for detection
- `struct Foo`, `enum Bar`, `type Alias = ...` — type definitions
- `impl Foo` and `impl Trait for Foo` — impl blocks (the chunker captures the entire block including all methods)
- `trait Foo` — trait definitions
- `mod foo` — modules (both inline `mod foo { }` and module declarations)
- `const FOO:`, `static FOO:` — constants and statics
- `macro_rules! foo` — declarative macros

Brace-depth tracking means the chunker opens `{` and closes `}` in sync, correctly handling nested structs, match arms, closures, and multi-line generics.

**What it does not parse:** attribute macros (`#[proc_macro]`), `use` statements, `extern crate`, `impl` blocks inside other `impl` blocks (treated as part of the outer block). These are all captured as part of the surrounding item.

### Python (`.py`)

The Python chunker uses indentation depth to find top-level items. It recognizes:

- `def foo(` and `async def foo(` — functions (indentation 0)
- `class Foo` — class definitions (indentation 0)
- Decorators — `@decorator` lines immediately before a `def` or `class` are included in the chunk

The chunker collects lines until the next top-level item starts (a line at indentation 0 that is itself a `def`, `class`, or decorator chain). Method definitions inside a class are part of the class chunk, not split out separately.

**What it does not parse:** nested `def` at non-zero indentation (captured inside the enclosing function/class), `if __name__ == '__main__'` blocks (treated as a top-level chunk with no symbol name), type stubs (`.pyi` files), or `@overload` detection.

### TypeScript (`.ts`, `.tsx`)

The TypeScript chunker uses brace-depth tracking (same approach as Rust) and is template-literal aware to avoid being misled by backtick strings containing `{`. It recognizes:

- `function foo(` and `async function foo(` — named function declarations
- `class Foo` — class declarations
- `interface Foo` — interface declarations
- `type Foo =` — type aliases
- `enum Foo` — enum declarations
- `const foo = ` with an arrow function `=>` or object body on the next line
- `let foo = `, `var foo = ` — variable declarations (top-level only)
- `export default function`, `export default class` — default exports

Template literals (backtick strings) that contain `{` are correctly handled — the chunker tracks backtick nesting depth and ignores `{` inside template literals.

**What it does not parse:** decorators (TypeScript class decorators), ambient module declarations (`declare module`), `namespace` blocks. These fall inside surrounding chunks.

### JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`)

Identical engine to TypeScript. The same brace-depth and template-literal logic applies. All the same constructs are recognized.

CommonJS (`module.exports = function ...`) is handled as a top-level `module` assignment.

### Go (`.go`)

The Go chunker uses brace-depth tracking with special handling for:

- `func Foo(` — bare functions
- `func (r ReceiverType) Method(` and `func (r *ReceiverType) Method(` — method receivers. The `symbol` field records the full form including the receiver type, e.g. `(Parser) Parse`.
- `type Foo struct`, `type Bar interface`, `type Alias = ...` — type declarations
- `var (...)` and `const (...)` — parenthesized blocks (collected as a single chunk)
- `var foo =` and `const foo =` — single-line var/const declarations

Paren blocks (`var ( ... )` and `const ( ... )`) are kept as a single chunk rather than split at each identifier.

**What it does not parse:** `init()` functions are treated as regular functions. Build constraints (`//go:build`) are part of the file header, not detected as separate items.

---

## Fallback Behavior for Unknown Extensions

For any file extension not listed above, `chunk_file` delegates to `split_on_blank_lines` — the same blank-line paragraph chunker that has always been used for markdown and plain text. The `language`, `symbol`, `signature`, and `line_range` fields are all `null` in those chunks.

This means you can safely call `chunk_file` on any file type without error. You just won't get symbol metadata for unsupported languages.

---

## Extended ChunkMeta Fields

Every chunk (from any language chunker or the markdown chunker) carries a `ChunkMeta` struct. In v0.8.0, four new fields were added:

| Field | Type | Markdown chunks | Code chunks |
|-------|------|----------------|-------------|
| `language` | `string \| null` | `null` | e.g. `"rust"`, `"python"`, `"go"` |
| `symbol` | `string \| null` | heading text (if any) | e.g. `"parse_error"`, `"(Parser) Parse"` |
| `signature` | `string \| null` | `null` | e.g. `"fn parse_error(input: &str) -> ParseError"` |
| `line_range` | `[start, end] \| null` | `null` | e.g. `[42, 89]` |

### Using These Fields in Search Results

When `memory.search` returns hits, each hit's `source_path` now points to the source file. To get the full text plus metadata for a hit, call `memory.fetch_chunk` with the chunk `id`. The response includes the full chunk text.

In practice, the LLM-facing workflow looks like:

1. **Search** — `memory.search({ query: "how is authentication handled" })` returns a list of hit IDs with scores and source paths.
2. **Fetch** — `memory.fetch_chunk({ id: "chunk-a1b2c3" })` returns the full function text.
3. **Use** — the LLM sees the complete function, its language, symbol name, signature, and line range in the response metadata.

### Example: Searching for a Function by Name

Suppose `auth.rs` contains:

```rust
pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims, AuthError> {
    let decoded = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret),
        &Validation::default(),
    )?;
    Ok(decoded.claims)
}
```

After indexing `auth.rs`, a search for `"token verification jwt claims"` will return this function as a hit. The chunk metadata will include:

```json
{
  "language": "rust",
  "symbol": "verify_token",
  "signature": "pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims, AuthError>",
  "line_range": [14, 22]
}
```

The LLM can immediately cite line 14 of `auth.rs` without having to count lines manually. It also knows the return type without reading the full body.

---

## How to Verify Code Chunking Is Working

The simplest check is to index a source file and then search for a symbol name that appears in it.

### Step 1: Add the source file as a doc

From an AI session:

```
docs.save({ path: "src/auth.rs", content: "<file contents>" })
```

Or, if you have auto-indexing enabled with the watcher watching your `docs/` directory, copy the file there.

### Step 2: Search by symbol name

```
memory.search({ query: "verify_token function signature" })
```

### Step 3: Fetch the hit

```
memory.fetch_chunk({ id: "<id from search result>" })
```

### What to look for

In the fetch response, check that:

- `language` is `"rust"` (or whichever language applies)
- `symbol` matches the function name (`"verify_token"`)
- `signature` shows the full first line of the function
- `line_range` gives `[start, end]` line numbers that match the source file

If `language` is `null` and `symbol` is `null`, the file extension is either unsupported or the chunker fell back to blank-line splitting. Check the extension is one of the supported ones listed above.

---

## Known Limitations

These chunkers are **regex-based**, not full AST parsers. This is an intentional tradeoff — regex chunkers are fast, zero-dependency, and handle the common 95% of real code well. But there are edge cases:

- **Multi-line function signatures** — if a function's parameter list spans many lines before the opening `{`, some chunkers may not capture the full signature in the `signature` field (they take the first line of the item).
- **String literals containing `{`** — the Rust and Go chunkers track brace depth, which means a raw string literal containing `{` could temporarily confuse the depth counter. This is rare in practice.
- **Deeply nested closures** — very complex lambda-heavy code (particularly JavaScript) may have chunking boundaries that don't align perfectly with logical boundaries.
- **Generated code** — minified or generated files tend to have very long lines with many symbols; chunk boundaries may not be meaningful.
- **`macro_rules!` complex bodies** — the macro body is captured as part of the macro chunk, but nested macro invocations inside the body can confuse brace tracking in unusual cases.
- **Python indentation quirks** — continuation lines (`\`) and multi-line strings that look like new `def` lines will not confuse the chunker, but deeply nested conditional expressions may cause oversized chunks.

**Future work:** a tree-sitter-based parser (planned for v0.9.0 or later) will resolve all of these edge cases by operating on the actual parse tree rather than text patterns.

---

## Chunk Size and Oversized Items

Individual language items (large classes, long impl blocks) that exceed the configured `max_chunk_tokens` limit are automatically split on blank lines within the item. The resulting sub-chunks still carry the same `language`, `symbol`, and `signature` as the original item, but the `line_range` is adjusted to reflect the sub-range.

The default limit is 1024 tokens (configurable via `max_chunk_tokens` in config). If a Go struct definition with 50 fields exceeds this limit, it will be split into multiple chunks, each tagged with the struct name.

---

## Configuration

No new configuration fields were added for code chunking in v0.8.0. The chunker is invoked automatically based on file extension whenever a source file is ingested. The existing `max_chunk_tokens` limit applies.

The only configuration relevant to code chunking is the standard document ingestion config:

```json
{
  "limits": {
    "max_chunk_tokens": 1024,
    "max_search_hits": 10
  }
}
```

---

## Dependency Added in v0.8.0

- `regex 1` — now a direct dependency of `the-one-memory` (was already a transitive dependency). Used by the language chunkers for pattern matching on top-level item boundaries.
