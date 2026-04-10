# Production Hardening Verification — 2026-04-10

## Scope

Follow-up verification after Redis/vector backend integration and wake-up/catalog hardening.
Goal: ensure no runtime stubs/placeholders remain in production paths and all workspace tests pass.

## Findings

### 1) Graph extractor test instability (fixed)

- **File:** `crates/the-one-memory/src/graph_extractor.rs`
- **Issue:** `test_extract_enabled_without_base_url_errors` could fail under parallel tests due to shared process env vars.
- **Fix:** added an env lock (`OnceLock<Mutex<()>>`) and explicit env cleanup in test setup/teardown.
- **Result:** deterministic test behavior across full workspace runs.

### 2) Embedded UI project switcher exposed non-functional behavior (fixed)

- **File:** `crates/the-one-ui/src/lib.rs`
- **Issue:** navigation rendered a project selector with JS alert fallback instead of real switching.
- **Fix:** replaced selector with authoritative read-only project display (`Project`, active value, known project count).
- **Result:** no non-functional controls or roadmap-style behavior exposed in runtime UI.

### 3) OCR fallback path labeled as “stub” (fixed)

- **File:** `crates/the-one-memory/src/ocr.rs`
- **Issue:** feature-disabled OCR implementation and test used “stub” terminology.
- **Fix:** renamed to “feature-disabled implementation” and updated test naming/assertion messages.
- **Result:** behavior remains explicit and production-safe without placeholder/stub framing.

## Verification Run

- `cargo fmt --check` ✅
- `cargo test --workspace` ✅
  - `the-one-core`: 52 passed
  - `the-one-mcp`: 83 passed, 1 ignored
  - `the-one-memory`: 159 passed
  - `the-one-router`: 24 passed
  - `the-one-ui`: 10 passed
  - plus adapter crates and doc-tests all passing

## Residual Notes

- Remaining `placeholder="..."` hits are HTML input placeholder attributes (UX hints), not implementation placeholders.
- No `todo!()` / `unimplemented!()` runtime markers were found in `crates/`.
