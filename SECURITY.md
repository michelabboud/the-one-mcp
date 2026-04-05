# Security Policy

## Reporting Vulnerabilities

If you discover a security vulnerability, please report it responsibly:

1. **Do NOT open a public GitHub issue**
2. Email: [create a private security advisory](../../security/advisories/new) on this repository
3. Include: description, reproduction steps, potential impact

We will respond within 48 hours and work with you on a fix.

## Security Design

### Tool Execution

- **No auto-install**: Tools are never installed without explicit user action (`tool.install`)
- **No auto-enable**: Tools must be explicitly enabled before they can run (`tool.enable`)
- **Risk-tier gating**: High-risk tools require approval scopes (once/session/forever)
- **Headless deny-by-default**: Automated environments deny high-risk actions unless prior approval exists
- **Audit trail**: Every `tool.run` is logged with timestamp and payload

### Data Isolation

- **Per-project**: Each project has its own SQLite database, manifests, and memory engine
- **No cross-project access**: Project data is keyed by `{project_root}::{project_id}`
- **Local-first**: All data stored locally — no telemetry, no phone-home, no cloud dependency

### Network Security

- **Qdrant strict auth**: Remote Qdrant connections require API key by default (`qdrant_strict_auth: true`)
- **TLS support**: Custom CA certificates and TLS verification for Qdrant connections
- **Fail-closed**: If strict auth is enabled and no API key is provided, the connection is refused
- **No external calls by default**: Local fastembed embeddings, rules-only routing — zero network calls unless explicitly configured

### Document Security

- **Path traversal prevention**: All doc paths validated — `../` rejected
- **Size limits**: Documents bounded by `max_doc_size_bytes` (default 100KB)
- **Count limits**: Managed folder bounded by `max_managed_docs` (default 500)
- **Safe characters**: Only alphanumeric, hyphens, underscores, dots, forward slashes in paths

### Image Ingestion Security

- **Path traversal prevention**: Image paths go through the same validation as doc paths — `../` sequences rejected
- **File size caps**: Images are bounded before loading into the embedding pipeline to prevent memory exhaustion
- **Format validation**: Only recognized image MIME types (PNG, JPEG, GIF, WebP, etc.) are accepted; arbitrary binary files are rejected
- **OCR is optional**: Tesseract OCR (`image-ocr` feature) is opt-in; disabled by default in the binary distribution — reducing the attack surface for malformed image inputs
- **Thumbnail scope**: Generated thumbnails are stored under the project's `.the-one/` directory, never outside the project boundary

### Catalog Security

- **Trust levels**: Every tool entry has a trust level: `verified`, `community`, `unverified`, `deprecated`, `warning`
- **Source tracking**: Catalog tools vs user tools are distinguished (`source: catalog | user`)
- **User tools are local-only**: `tool.add` stores tools locally, never uploads to the catalog
- **Install commands are visible**: The LLM and user can inspect `install_command` before running `tool.install`

### Configuration Security

- **No secrets in config files**: API keys should use environment variables (`THE_ONE_QDRANT_API_KEY`, `THE_ONE_EMBEDDING_API_KEY`)
- **Config files are local**: `~/.the-one/config.json` and `<project>/.the-one/config.json` are not committed to git (`.the-one` is gitignored)
- **Environment variable precedence**: Env vars override file config — secrets never need to be written to disk

## Supported Versions

| Version | Supported |
|---------|-----------|
| v0.6.x | Current — active development |
| v0.5.x | Security fixes only |
| < v0.5.0 | Not supported |

## Dependencies

Key dependencies and their security posture:

| Dependency | Purpose | Notes |
|-----------|---------|-------|
| `rusqlite` (bundled) | SQLite storage | Bundled SQLite, no system dependency |
| `fastembed` 5.x / `ort` | ONNX text embeddings | Downloads model from Hugging Face on first use |
| `image` (optional) | Image processing | Required by `image-embeddings` feature (default on) |
| `tesseract` (optional) | OCR text extraction | Required by `image-ocr` feature; system `tesseract` binary must be present |
| `reqwest` | HTTP client | Used for Qdrant, nano providers, API embeddings |
| `axum` | HTTP server | Used for SSE/stream transports and admin UI |
| `tokio` | Async runtime | Standard Rust async |

### Model Downloads

The `fastembed` 5.x provider downloads ONNX models from Hugging Face Hub on first use. Models are cached in `~/.the-one/.fastembed_cache/`. To avoid runtime downloads in restricted environments:

1. Pre-download the model on a trusted machine
2. Copy `.fastembed_cache/` to the target machine
3. Or use `embedding_provider: "api"` to skip local models entirely

Image embedding models (CLIP-based) are downloaded separately and cached alongside text models.
