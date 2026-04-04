#!/usr/bin/env bash
set -euo pipefail

# ╔══════════════════════════════════════════════════════════════════════════╗
# ║  THE-ONE MCP — Installer                                               ║
# ║  One command to download, install, configure, and connect.              ║
# ╚══════════════════════════════════════════════════════════════════════════╝
#
# ┌────────────────────────────────────────────────────────────────────────┐
# │  What This Does                                                        │
# │                                                                        │
# │  1. Detect your OS and architecture                                    │
# │  2. Download the latest release (or use local build / specific ver)    │
# │  3. Install binaries to ~/.the-one/bin/                                │
# │  4. Create default config (if none exists)                             │
# │  5. Download recommended tools catalog                                 │
# │  6. Detect Claude Code / Codex and register the MCP server             │
# │  7. Validate the installation with a smoke test                        │
# │                                                                        │
# │  After install, the MCP works with zero additional configuration.      │
# │  Local embeddings (offline, free), rules-only routing, keyword search. │
# ├────────────────────────────────────────────────────────────────────────┤
# │  Install Layout                                                        │
# │                                                                        │
# │  ~/.the-one/                                                           │
# │  ├── bin/                                                              │
# │  │   ├── the-one-mcp       MCP server binary                          │
# │  │   └── embedded-ui       Admin UI binary                             │
# │  ├── config.json           Global config (defaults, editable)          │
# │  ├── registry/                                                         │
# │  │   ├── recommended.json  Pre-built tools (auto-updated)              │
# │  │   └── custom.json       Your own tools (never overwritten)          │
# │  └── .fastembed_cache/     ONNX model (auto-downloaded on first use)  │
# ├────────────────────────────────────────────────────────────────────────┤
# │  Usage                                                                 │
# │                                                                        │
# │  curl -fsSL <raw-url>/scripts/install.sh | bash                        │
# │  bash install.sh                          # latest release             │
# │  bash install.sh --version v0.2.0         # specific version           │
# │  bash install.sh --local ./target/release # from local build           │
# │  bash install.sh --lean                   # no-swagger binary          │
# │  bash install.sh --uninstall              # remove everything          │
# └────────────────────────────────────────────────────────────────────────┘

# ── Constants ──────────────────────────────────────────────────────────────
readonly GITHUB_REPO="michelabboud/the-one-mcp"
readonly GITHUB_API="https://api.github.com/repos/${GITHUB_REPO}"
readonly GITHUB_RAW="https://raw.githubusercontent.com/${GITHUB_REPO}/main"
readonly INSTALL_DIR="${THE_ONE_HOME:-${HOME}/.the-one}"
readonly BIN_DIR="${INSTALL_DIR}/bin"
readonly REGISTRY_DIR="${INSTALL_DIR}/registry"
readonly CONFIG_FILE="${INSTALL_DIR}/config.json"
readonly BIN_NAME="the-one-mcp"
readonly UI_BIN_NAME="embedded-ui"

# ── Options ────────────────────────────────────────────────────────────────
VERSION=""
LOCAL_DIR=""
LEAN=false
UNINSTALL=false
SKIP_REGISTER=false
SKIP_VALIDATE=false
YES=false
NO_COLOR=false
VERBOSE=false

# ── Colors ─────────────────────────────────────────────────────────────────
setup_colors() {
    if [ "$NO_COLOR" = true ] || [ ! -t 1 ]; then
        RED='' GREEN='' YELLOW='' BLUE='' CYAN='' DIM='' BOLD='' NC=''
    else
        RED=$'\033[0;31m'
        GREEN=$'\033[0;32m'
        YELLOW=$'\033[1;33m'
        BLUE=$'\033[0;34m'
        CYAN=$'\033[0;36m'
        DIM=$'\033[2m'
        BOLD=$'\033[1m'
        NC=$'\033[0m'
    fi
}

log()     { echo -e "${GREEN}[INSTALL]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
err()     { echo -e "${RED}[ERROR]${NC} $*" >&2; }
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
debug()   { [ "$VERBOSE" = true ] && echo -e "${DIM}[DEBUG] $*${NC}" || true; }
ok()      { echo -e "  ${GREEN}✓${NC} $*"; }
fail()    { echo -e "  ${RED}✗${NC} $*"; }
skip()    { echo -e "  ${DIM}–${NC} $*"; }

confirm() {
    if [ "$YES" = true ]; then return 0; fi
    local prompt="${1:-Continue?}"
    echo -en "${YELLOW}${prompt} [Y/n] ${NC}"
    read -r response
    [[ -z "$response" || "$response" =~ ^[Yy] ]]
}

# ── OS / Arch Detection ───────────────────────────────────────────────────
detect_platform() {
    local os arch

    case "$(uname -s)" in
        Linux*)  os="linux" ;;
        Darwin*) os="macos" ;;
        MINGW*|MSYS*|CYGWIN*) os="windows" ;;
        *)
            err "Unsupported OS: $(uname -s)"
            err "Supported: Linux, macOS, Windows (via Git Bash/MSYS2)"
            exit 1
            ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)  arch="aarch64" ;;
        *)
            err "Unsupported architecture: $(uname -m)"
            err "Supported: x86_64 (amd64), aarch64 (arm64)"
            exit 1
            ;;
    esac

    PLATFORM_OS="$os"
    PLATFORM_ARCH="$arch"
    PLATFORM_NAME="${os}-${arch}"

    if [ "$os" = "windows" ]; then
        PLATFORM_EXT=".exe"
        ARCHIVE_EXT="zip"
    else
        PLATFORM_EXT=""
        ARCHIVE_EXT="tar.gz"
    fi

    debug "Detected platform: ${PLATFORM_NAME} (ext: ${PLATFORM_EXT})"
}

# ── Detect CLI Tools ──────────────────────────────────────────────────────
detect_clis() {
    HAS_CLAUDE=false
    HAS_CODEX=false
    HAS_GEMINI=false
    HAS_OPENCODE=false
    HAS_CURL=false
    HAS_GH=false

    CLAUDE_VERSION=""
    CODEX_VERSION=""
    GEMINI_VERSION=""
    OPENCODE_VERSION=""

    if command -v claude >/dev/null 2>&1; then
        HAS_CLAUDE=true
        CLAUDE_VERSION=$(claude --version 2>/dev/null | head -1 || echo "unknown")
    fi
    if command -v codex >/dev/null 2>&1; then
        HAS_CODEX=true
        CODEX_VERSION=$(codex --version 2>/dev/null | head -1 || echo "unknown")
    fi
    if command -v gemini >/dev/null 2>&1; then
        HAS_GEMINI=true
        GEMINI_VERSION=$(gemini --version 2>/dev/null | head -1 || echo "unknown")
    fi
    if command -v opencode >/dev/null 2>&1; then
        HAS_OPENCODE=true
        OPENCODE_VERSION=$(opencode --version 2>/dev/null | head -1 || echo "unknown")
    fi

    command -v curl >/dev/null 2>&1 && HAS_CURL=true
    command -v gh >/dev/null 2>&1 && HAS_GH=true

    # wget fallback
    if [ "$HAS_CURL" = false ]; then
        if command -v wget >/dev/null 2>&1; then
            HAS_WGET=true
        else
            err "Neither curl nor wget found. Please install curl."
            exit 1
        fi
    fi
}

# ── Download Helper ────────────────────────────────────────────────────────
download() {
    local url="$1"
    local dest="$2"
    debug "Downloading: $url -> $dest"
    if [ "$HAS_CURL" = true ]; then
        curl -fsSL "$url" -o "$dest"
    else
        wget -q "$url" -O "$dest"
    fi
}

download_to_stdout() {
    local url="$1"
    if [ "$HAS_CURL" = true ]; then
        curl -fsSL "$url"
    else
        wget -q "$url" -O -
    fi
}

# ── Resolve Version ───────────────────────────────────────────────────────
resolve_version() {
    if [ -n "$VERSION" ]; then
        debug "Using specified version: $VERSION"
        return 0
    fi

    info "Fetching latest release..."
    local latest
    latest=$(download_to_stdout "${GITHUB_API}/releases/latest" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

    if [ -z "$latest" ]; then
        err "Failed to determine latest version from GitHub"
        err "Try specifying a version: install.sh --version v0.2.0"
        exit 1
    fi

    VERSION="$latest"
    log "Latest version: ${VERSION}"
}

# ── Download Release ──────────────────────────────────────────────────────
download_release() {
    local suffix="${LEAN:+lean-}${PLATFORM_NAME}"
    local archive_name="the-one-mcp-${VERSION}-${PLATFORM_NAME}.${ARCHIVE_EXT}"
    local download_url="https://github.com/${GITHUB_REPO}/releases/download/${VERSION}/${archive_name}"

    local tmp_dir
    tmp_dir=$(mktemp -d)
    trap "rm -rf '$tmp_dir'" EXIT

    info "Downloading ${archive_name}..."
    download "$download_url" "${tmp_dir}/${archive_name}"

    info "Extracting..."
    cd "$tmp_dir"
    if [ "$ARCHIVE_EXT" = "zip" ]; then
        unzip -q "${archive_name}"
    else
        tar -xzf "${archive_name}"
    fi

    # Find the extracted directory
    local extract_dir
    extract_dir=$(find . -maxdepth 1 -type d -name "the-one-mcp-*" | head -1)
    if [ -z "$extract_dir" ]; then
        err "Extraction failed — no the-one-mcp-* directory found"
        exit 1
    fi

    # Copy binaries
    mkdir -p "$BIN_DIR"
    local bin_src
    if [ "$LEAN" = true ]; then
        bin_src="${extract_dir}/${BIN_NAME}-lean${PLATFORM_EXT}"
    else
        bin_src="${extract_dir}/${BIN_NAME}${PLATFORM_EXT}"
    fi

    if [ ! -f "$bin_src" ]; then
        err "Binary not found in archive: $bin_src"
        exit 1
    fi

    cp "$bin_src" "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"
    chmod +x "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"

    # Also install UI binary if present
    local ui_src="${extract_dir}/${UI_BIN_NAME}${PLATFORM_EXT}"
    if [ -f "$ui_src" ]; then
        cp "$ui_src" "${BIN_DIR}/${UI_BIN_NAME}${PLATFORM_EXT}"
        chmod +x "${BIN_DIR}/${UI_BIN_NAME}${PLATFORM_EXT}"
    fi

    # Copy schemas if present
    if [ -d "${extract_dir}/schemas" ]; then
        cp -r "${extract_dir}/schemas" "${INSTALL_DIR}/"
    fi

    cd - >/dev/null
    ok "Binaries installed to ${BIN_DIR}/"
}

# ── Install from Local Build ──────────────────────────────────────────────
install_local() {
    local src_dir="$LOCAL_DIR"

    local bin_src
    if [ "$LEAN" = true ]; then
        bin_src="${src_dir}/${BIN_NAME}-lean${PLATFORM_EXT}"
        if [ ! -f "$bin_src" ]; then
            bin_src="${src_dir}/${BIN_NAME}${PLATFORM_EXT}"
            warn "Lean binary not found, using standard binary"
        fi
    else
        bin_src="${src_dir}/${BIN_NAME}${PLATFORM_EXT}"
    fi

    if [ ! -f "$bin_src" ]; then
        err "Binary not found: $bin_src"
        err "Build first: bash scripts/build.sh build"
        exit 1
    fi

    mkdir -p "$BIN_DIR"
    cp "$bin_src" "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"
    chmod +x "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"

    # UI binary
    local ui_src="${src_dir}/${UI_BIN_NAME}${PLATFORM_EXT}"
    if [ -f "$ui_src" ]; then
        cp "$ui_src" "${BIN_DIR}/${UI_BIN_NAME}${PLATFORM_EXT}"
        chmod +x "${BIN_DIR}/${UI_BIN_NAME}${PLATFORM_EXT}"
    fi

    ok "Binaries installed from local build"
}

# ── Create Default Config ─────────────────────────────────────────────────
create_default_config() {
    if [ -f "$CONFIG_FILE" ]; then
        ok "Config exists: ${CONFIG_FILE} (preserved)"
        return 0
    fi

    cat > "$CONFIG_FILE" <<'CONFIGEOF'
{
  "provider": "local",
  "log_level": "info",
  "embedding_provider": "local",
  "embedding_model": "BGE-large-en-v1.5",
  "embedding_dimensions": 1024,
  "qdrant_url": "http://127.0.0.1:6334",
  "qdrant_strict_auth": true,
  "nano_routing_policy": "priority",
  "nano_providers": [],
  "limits": {
    "max_tool_suggestions": 5,
    "max_search_hits": 5,
    "max_raw_section_bytes": 24576,
    "max_enabled_families": 12,
    "max_doc_size_bytes": 102400,
    "max_managed_docs": 500,
    "max_embedding_batch_size": 64,
    "max_chunk_tokens": 512,
    "max_nano_timeout_ms": 2000,
    "max_nano_retries": 3,
    "max_nano_providers": 5,
    "search_score_threshold": 0.3
  }
}
CONFIGEOF

    ok "Default config created: ${CONFIG_FILE}"
}

# ── Embedding Model Selection ─────────────────────────────────────────────
select_embedding_model() {
    # Skip in non-interactive mode
    if [ "$YES" = true ] || [ ! -t 0 ]; then
        info "Using default embedding model: BGE-large-en-v1.5 (quality)"
        return 0
    fi

    echo ""
    echo "${CYAN}${BOLD}╔══════════════════════════════════════════════════════════════════════════╗${NC}"
    echo "${CYAN}${BOLD}║  Embedding Model Selection                                              ║${NC}"
    echo "${CYAN}${BOLD}╠══════════════════════════════════════════════════════════════════════════╣${NC}"
    echo "${CYAN}${BOLD}║                                                                          ║${NC}"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}#${NC}   ${BOLD}%-26s${NC} ${DIM}%-6s %-7s %-14s %-12s${NC}${CYAN}${BOLD}║${NC}\n" \
        "Model" "Dims" "Size" "Latency" "Multilingual"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}%-3s${NC} ${DIM}%-26s %-6s %-7s %-14s %-12s${NC}${CYAN}${BOLD}║${NC}\n" \
        "───" "──────────────────────────" "──────" "───────" "──────────────" "────────────"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}1${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "all-MiniLM-L6-v2" "384" "23MB" "fastest" "No"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}2${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "BGE-base-en-v1.5" "768" "50MB" "~2x slower" "No"
    printf "${CYAN}${BOLD}║${NC} ${GREEN}[3]${NC}  ${BOLD}%-22s${NC} ${GREEN}★${NC}  %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "BGE-large-en-v1.5" "1024" "130MB" "~4x slower" "No"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}4${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "multilingual-e5-large" "1024" "220MB" "~5x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}5${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "multilingual-e5-base" "768" "90MB" "~3x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}6${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "multilingual-e5-small" "384" "45MB" "~1.5x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}7${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "paraphrase-ml-minilm" "384" "45MB" "~1.5x slower" "Yes"
    printf "${CYAN}${BOLD}║${NC}  ${DIM}8${NC}   %-26s %-6s %-7s %-14s %-12s${CYAN}${BOLD}║${NC}\n" \
        "API (OpenAI/Voyage/Cohere)" "—" "—" "—" "Depends"
    echo "${CYAN}${BOLD}║                                                                          ║${NC}"
    echo "${CYAN}${BOLD}║  ${GREEN}★${NC} ${DIM}= recommended${NC}                                                         ${CYAN}${BOLD}║${NC}"
    echo "${CYAN}${BOLD}╚══════════════════════════════════════════════════════════════════════════╝${NC}"
    echo ""

    echo -en "${YELLOW}Select model [3]: ${NC}"
    read -r model_choice

    # Default to 3 if empty
    model_choice="${model_choice:-3}"

    local model_name dims
    case "$model_choice" in
        1) model_name="all-MiniLM-L6-v2"; dims=384 ;;
        2) model_name="BGE-base-en-v1.5"; dims=768 ;;
        3) model_name="BGE-large-en-v1.5"; dims=1024 ;;
        4) model_name="multilingual-e5-large"; dims=1024 ;;
        5) model_name="multilingual-e5-base"; dims=768 ;;
        6) model_name="multilingual-e5-small"; dims=384 ;;
        7) model_name="paraphrase-ml-minilm-l12-v2"; dims=384 ;;
        8)
            select_api_model
            return $?
            ;;
        *)
            warn "Invalid choice '$model_choice', using default (BGE-large-en-v1.5)"
            model_name="BGE-large-en-v1.5"; dims=1024
            ;;
    esac

    # Update config.json with chosen model
    update_config_model "local" "$model_name" "$dims" "" ""
    ok "Selected: ${model_name} (${dims}d)"
}

select_api_model() {
    echo ""
    echo "${BOLD}API Provider:${NC}"
    echo "  ${DIM}1${NC}  OpenAI"
    echo "  ${DIM}2${NC}  Voyage AI"
    echo "  ${DIM}3${NC}  Cohere"
    echo "  ${DIM}4${NC}  Custom (enter base URL)"
    echo ""
    echo -en "${YELLOW}Select provider [1]: ${NC}"
    read -r provider_choice
    provider_choice="${provider_choice:-1}"

    local provider_name base_url default_env default_model default_dims
    case "$provider_choice" in
        1)
            provider_name="OpenAI"
            base_url="https://api.openai.com/v1"
            default_env="OPENAI_API_KEY"
            default_model="text-embedding-3-small"
            default_dims=1536
            ;;
        2)
            provider_name="Voyage AI"
            base_url="https://api.voyageai.com/v1"
            default_env="VOYAGE_API_KEY"
            default_model="voyage-3"
            default_dims=1024
            ;;
        3)
            provider_name="Cohere"
            base_url="https://api.cohere.com/v2"
            default_env="COHERE_API_KEY"
            default_model="embed-v4.0"
            default_dims=1024
            ;;
        4)
            echo -en "${YELLOW}Base URL: ${NC}"
            read -r base_url
            default_env=""
            default_model=""
            default_dims=1536
            provider_name="Custom"
            ;;
        *)
            warn "Invalid choice, using OpenAI"
            provider_name="OpenAI"
            base_url="https://api.openai.com/v1"
            default_env="OPENAI_API_KEY"
            default_model="text-embedding-3-small"
            default_dims=1536
            ;;
    esac

    echo ""
    echo -en "${YELLOW}API Key (or env var name) [${default_env}]: ${NC}"
    read -r api_key
    api_key="${api_key:-$default_env}"

    echo -en "${YELLOW}Model [${default_model}]: ${NC}"
    read -r model_name
    model_name="${model_name:-$default_model}"

    echo -en "${YELLOW}Dimensions [${default_dims}]: ${NC}"
    read -r dims
    dims="${dims:-$default_dims}"

    update_config_model "api" "$model_name" "$dims" "$base_url" "$api_key"
    ok "Selected: ${provider_name} / ${model_name} (${dims}d)"
}

update_config_model() {
    local provider="$1" model="$2" dims="$3" base_url="$4" api_key="$5"

    if [ ! -f "$CONFIG_FILE" ]; then
        warn "Config file not found, skipping model update"
        return 1
    fi

    # Use a temp file for atomic update
    local tmp_file="${CONFIG_FILE}.tmp"

    if command -v python3 &>/dev/null; then
        python3 -c "
import json, sys
with open('${CONFIG_FILE}') as f:
    config = json.load(f)
config['embedding_provider'] = '${provider}'
config['embedding_model'] = '${model}'
config['embedding_dimensions'] = ${dims}
if '${base_url}':
    config['embedding_api_base_url'] = '${base_url}'
if '${api_key}':
    config['embedding_api_key'] = '${api_key}'
with open('${tmp_file}', 'w') as f:
    json.dump(config, f, indent=2)
    f.write('\n')
" && mv "$tmp_file" "$CONFIG_FILE"
    else
        # Fallback: sed-based replacement (less robust but works without python)
        sed -i "s/\"embedding_provider\": \"[^\"]*\"/\"embedding_provider\": \"${provider}\"/" "$CONFIG_FILE"
        sed -i "s/\"embedding_model\": \"[^\"]*\"/\"embedding_model\": \"${model}\"/" "$CONFIG_FILE"
        if grep -q '"embedding_dimensions"' "$CONFIG_FILE"; then
            sed -i "s/\"embedding_dimensions\": [0-9]*/\"embedding_dimensions\": ${dims}/" "$CONFIG_FILE"
        fi
    fi
}

# ── Download Recommended Tools ─────────────────────────────────────────────
download_recommended_tools() {
    mkdir -p "$REGISTRY_DIR"

    info "Downloading recommended tools catalog..."
    if download "${GITHUB_RAW}/tools/recommended.json" "${REGISTRY_DIR}/recommended.json" 2>/dev/null; then
        ok "Recommended tools: ${REGISTRY_DIR}/recommended.json"
    else
        warn "Failed to download recommended tools (offline install). Using bundled fallback."
        # Minimal fallback
        cat > "${REGISTRY_DIR}/recommended.json" <<'TOOLSEOF'
[
  {"id":"project.init","title":"Initialize Project","capability_type":"McpTool","family":"project","visibility_mode":"Core","risk_level":"Low","description":"Detect project and create state."},
  {"id":"memory.search","title":"Search Memory","capability_type":"McpTool","family":"memory","visibility_mode":"Core","risk_level":"Low","description":"Semantic search over indexed docs."},
  {"id":"docs.create","title":"Create Document","capability_type":"McpTool","family":"docs","visibility_mode":"Core","risk_level":"Low","description":"Create a managed markdown document."},
  {"id":"docs.list","title":"List Documents","capability_type":"McpTool","family":"docs","visibility_mode":"Core","risk_level":"Low","description":"List all indexed documentation files."}
]
TOOLSEOF
    fi

    # Create custom tools file if it doesn't exist
    if [ ! -f "${REGISTRY_DIR}/custom.json" ]; then
        echo "[]" > "${REGISTRY_DIR}/custom.json"
        ok "Custom tools file: ${REGISTRY_DIR}/custom.json (empty, add your own)"
    else
        ok "Custom tools preserved: ${REGISTRY_DIR}/custom.json"
    fi
}

# ── Register with AI Assistants ────────────────────────────────────────────

register_all_clis() {
    if [ "$SKIP_REGISTER" = true ]; then
        skip "Skipping CLI registration (--skip-register)"
        return 0
    fi

    register_claude_code
    register_gemini_cli
    register_opencode
    register_codex
}

register_claude_code() {
    if [ "$HAS_CLAUDE" = false ]; then
        skip "Claude Code not found"
        return 0
    fi

    info "Detected Claude Code (${CLAUDE_VERSION})"
    if confirm "  Register the-one-mcp with Claude Code?"; then
        if claude mcp add "${BIN_NAME}" -- "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}" serve 2>/dev/null; then
            ok "Registered with Claude Code"
        else
            warn "Auto-registration failed. Add manually:"
            echo "    claude mcp add ${BIN_NAME} -- ${BIN_DIR}/${BIN_NAME} serve"
        fi
    else
        skip "Skipped Claude Code"
        echo "    claude mcp add ${BIN_NAME} -- ${BIN_DIR}/${BIN_NAME} serve"
    fi
}

register_gemini_cli() {
    if [ "$HAS_GEMINI" = false ]; then
        skip "Gemini CLI not found"
        return 0
    fi

    info "Detected Gemini CLI (${GEMINI_VERSION})"
    if confirm "  Register the-one-mcp with Gemini CLI?"; then
        if gemini mcp add "${BIN_NAME}" "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}" serve 2>/dev/null; then
            ok "Registered with Gemini CLI"
        else
            # Fallback: write to settings.json directly
            local gemini_settings="${HOME}/.gemini/settings.json"
            if [ -f "$gemini_settings" ]; then
                # Check if already registered
                if grep -q "\"${BIN_NAME}\"" "$gemini_settings" 2>/dev/null; then
                    ok "Already registered in Gemini settings"
                    return 0
                fi

                # Use a temp file to inject MCP config
                local tmp_settings
                tmp_settings=$(mktemp)
                python3 -c "
import json, sys
with open('$gemini_settings') as f:
    cfg = json.load(f)
cfg.setdefault('mcpServers', {})
cfg['mcpServers']['${BIN_NAME}'] = {
    'command': '${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}',
    'args': ['serve']
}
with open('$tmp_settings', 'w') as f:
    json.dump(cfg, f, indent=2)
" 2>/dev/null && mv "$tmp_settings" "$gemini_settings" && ok "Registered in Gemini settings.json" || {
                    rm -f "$tmp_settings"
                    warn "Auto-registration failed. Add manually to ~/.gemini/settings.json:"
                    echo "    \"mcpServers\": { \"${BIN_NAME}\": { \"command\": \"${BIN_DIR}/${BIN_NAME}\", \"args\": [\"serve\"] } }"
                }
            else
                warn "Gemini settings not found. Add manually:"
                echo "    gemini mcp add ${BIN_NAME} ${BIN_DIR}/${BIN_NAME} serve"
            fi
        fi
    else
        skip "Skipped Gemini CLI"
        echo "    gemini mcp add ${BIN_NAME} ${BIN_DIR}/${BIN_NAME} serve"
    fi
}

register_opencode() {
    if [ "$HAS_OPENCODE" = false ]; then
        skip "OpenCode not found"
        return 0
    fi

    info "Detected OpenCode (${OPENCODE_VERSION})"
    if confirm "  Register the-one-mcp with OpenCode?"; then
        if opencode mcp add --name "${BIN_NAME}" --command "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}" --args serve 2>/dev/null; then
            ok "Registered with OpenCode"
        else
            # Try alternative syntax
            if opencode mcp add "${BIN_NAME}" --command "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}" --args serve 2>/dev/null; then
                ok "Registered with OpenCode"
            else
                warn "Auto-registration failed. Add manually:"
                echo "    opencode mcp add --name ${BIN_NAME} --command ${BIN_DIR}/${BIN_NAME} --args serve"
            fi
        fi
    else
        skip "Skipped OpenCode"
        echo "    opencode mcp add --name ${BIN_NAME} --command ${BIN_DIR}/${BIN_NAME} --args serve"
    fi
}

register_codex() {
    if [ "$HAS_CODEX" = false ]; then
        skip "Codex not found"
        return 0
    fi

    info "Detected Codex (${CODEX_VERSION})"
    echo "    To configure, add to your Codex MCP config:"
    echo "    ${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT} serve"
}

# ── Update PATH ───────────────────────────────────────────────────────────
ensure_path() {
    if echo "$PATH" | tr ':' '\n' | grep -q "^${BIN_DIR}$"; then
        debug "BIN_DIR already in PATH"
        return 0
    fi

    local shell_name
    shell_name=$(basename "${SHELL:-/bin/bash}")
    local rc_file=""

    case "$shell_name" in
        bash) rc_file="${HOME}/.bashrc" ;;
        zsh)  rc_file="${HOME}/.zshrc" ;;
        fish) rc_file="${HOME}/.config/fish/config.fish" ;;
    esac

    if [ -n "$rc_file" ]; then
        local path_line="export PATH=\"${BIN_DIR}:\$PATH\""
        if [ "$shell_name" = "fish" ]; then
            path_line="set -gx PATH ${BIN_DIR} \$PATH"
        fi

        # Check if already added
        if [ -f "$rc_file" ] && grep -q "the-one" "$rc_file" 2>/dev/null; then
            debug "PATH entry already in $rc_file"
            return 0
        fi

        if confirm "  Add ${BIN_DIR} to PATH in ${rc_file}?"; then
            echo "" >> "$rc_file"
            echo "# the-one-mcp" >> "$rc_file"
            echo "$path_line" >> "$rc_file"
            ok "Added to PATH in ${rc_file}"
            warn "Run 'source ${rc_file}' or open a new terminal for PATH to take effect"
        else
            skip "PATH not updated. Add manually: ${path_line}"
        fi
    else
        warn "Could not detect shell config file. Add to PATH manually:"
        echo "    export PATH=\"${BIN_DIR}:\$PATH\""
    fi
}

# ── Validate Installation ─────────────────────────────────────────────────
validate_install() {
    if [ "$SKIP_VALIDATE" = true ]; then
        skip "Skipping validation (--skip-validate)"
        return 0
    fi

    echo ""
    info "Validating installation..."

    local mcp_bin="${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"
    local all_ok=true

    # Check binary exists and is executable
    if [ -x "$mcp_bin" ]; then
        ok "Binary exists and is executable"
    else
        fail "Binary not found or not executable: $mcp_bin"
        all_ok=false
    fi

    # Check config exists
    if [ -f "$CONFIG_FILE" ]; then
        ok "Config file exists"
    else
        fail "Config file missing: $CONFIG_FILE"
        all_ok=false
    fi

    # Check recommended tools
    if [ -f "${REGISTRY_DIR}/recommended.json" ]; then
        local tool_count
        tool_count=$(grep -c '"id"' "${REGISTRY_DIR}/recommended.json" 2>/dev/null || echo "0")
        ok "Recommended tools: ${tool_count} tools loaded"
    else
        fail "Recommended tools missing"
        all_ok=false
    fi

    # Smoke test: send initialize request
    if [ "$all_ok" = true ]; then
        info "Running smoke test..."
        local response
        response=$(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | "$mcp_bin" serve 2>/dev/null | head -1)

        if echo "$response" | grep -q '"the-one-mcp"' 2>/dev/null; then
            ok "Smoke test passed: MCP server responds correctly"
        else
            warn "Smoke test inconclusive (server may need a project context)"
            debug "Response: $response"
        fi

        # Test tools/list
        local tools_response
        tools_response=$(echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' | "$mcp_bin" serve 2>/dev/null | head -2 | tail -1)

        if echo "$tools_response" | grep -q '"tools"' 2>/dev/null; then
            local tool_count
            tool_count=$(echo "$tools_response" | grep -o '"name"' | wc -l)
            ok "Tools available: ${tool_count} MCP tools"
        fi
    fi

    # Check Claude Code registration
    if [ "$HAS_CLAUDE" = true ]; then
        if claude mcp list 2>/dev/null | grep -q "${BIN_NAME}" 2>/dev/null; then
            ok "Registered with Claude Code"
        else
            skip "Not yet registered with Claude Code"
        fi
    fi

    echo ""
    if [ "$all_ok" = true ]; then
        log "${GREEN}${BOLD}Installation validated successfully${NC}"
    else
        warn "Installation completed with warnings. Check the issues above."
    fi
}

# ── Uninstall ──────────────────────────────────────────────────────────────
cmd_uninstall() {
    echo ""
    warn "This will remove:"
    echo "    ${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"
    echo "    ${BIN_DIR}/${UI_BIN_NAME}${PLATFORM_EXT}"
    echo ""
    info "The following will be ${BOLD}preserved${NC}:"
    echo "    ${CONFIG_FILE}          (your config)"
    echo "    ${REGISTRY_DIR}/custom.json   (your custom tools)"
    echo ""
    info "To remove everything: rm -rf ${INSTALL_DIR}"
    echo ""

    if ! confirm "Remove binaries and recommended tools?"; then
        log "Uninstall cancelled"
        exit 0
    fi

    # Unregister from Claude Code
    if [ "$HAS_CLAUDE" = true ]; then
        if claude mcp list 2>/dev/null | grep -q "${BIN_NAME}" 2>/dev/null; then
            claude mcp remove "${BIN_NAME}" 2>/dev/null && ok "Unregistered from Claude Code" || true
        fi
    fi

    # Remove binaries
    rm -f "${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"
    rm -f "${BIN_DIR}/${UI_BIN_NAME}${PLATFORM_EXT}"
    ok "Binaries removed"

    # Remove recommended tools (preserve custom)
    rm -f "${REGISTRY_DIR}/recommended.json"
    ok "Recommended tools removed (custom.json preserved)"

    # Remove schemas
    rm -rf "${INSTALL_DIR}/schemas"

    # Clean empty dirs
    rmdir "${BIN_DIR}" 2>/dev/null || true

    log "Uninstall complete"
    info "Config and custom tools preserved at: ${INSTALL_DIR}/"
    info "To remove everything: rm -rf ${INSTALL_DIR}"
}

# ── Help ──────────────────────────────────────────────────────────────────
cmd_help() {
    cat <<EOF
${CYAN}${BOLD}THE-ONE MCP — Installer${NC}

One command to download, install, configure, and connect.
After install, the MCP works with zero additional configuration.

${CYAN}${BOLD}USAGE${NC}
    bash install.sh [options]
    curl -fsSL <raw-url>/scripts/install.sh | bash

${CYAN}${BOLD}OPTIONS${NC}
    --version <ver>      Install specific version (e.g., v0.2.0)
    --local <dir>        Install from local build directory
    --lean               Use the lean binary (no swagger, smaller)
    --skip-register      Don't register with Claude Code / Codex
    --skip-validate      Don't run validation after install
    --uninstall          Remove binaries and recommended tools
    --yes                Skip all confirmation prompts
    --no-color           Disable colored output
    --verbose            Show debug output
    --help               Show this help

${CYAN}${BOLD}EXAMPLES${NC}
    ${GREEN}# Install latest release (recommended):${NC}
    bash install.sh

    ${GREEN}# Install specific version:${NC}
    bash install.sh --version v0.2.0

    ${GREEN}# Install from local build (after running build.sh):${NC}
    bash install.sh --local ./target/release

    ${GREEN}# Install lean binary (no swagger):${NC}
    bash install.sh --lean

    ${GREEN}# Non-interactive install (CI/automation):${NC}
    bash install.sh --yes --skip-register

    ${GREEN}# Uninstall:${NC}
    bash install.sh --uninstall

    ${GREEN}# Pipe install (one-liner):${NC}
    curl -fsSL https://raw.githubusercontent.com/${GITHUB_REPO}/main/scripts/install.sh | bash

${CYAN}${BOLD}WHAT GETS INSTALLED${NC}
    ~/.the-one/
    ├── bin/
    │   ├── the-one-mcp         MCP server binary
    │   └── embedded-ui         Admin UI binary
    ├── config.json             Default config (editable)
    ├── registry/
    │   ├── recommended.json    Pre-built tools (from GitHub)
    │   └── custom.json         Your own tools (never overwritten)
    └── schemas/                v1beta JSON schemas

${CYAN}${BOLD}AFTER INSTALL${NC}
    ${GREEN}# The MCP is already registered with Claude Code (if detected).${NC}
    ${GREEN}# Just start a Claude Code session — it connects automatically.${NC}

    ${GREEN}# Or run manually:${NC}
    the-one-mcp serve

    ${GREEN}# Customize config:${NC}
    \$EDITOR ~/.the-one/config.json

    ${GREEN}# Add custom tools:${NC}
    \$EDITOR ~/.the-one/registry/custom.json

    ${GREEN}# Run admin UI:${NC}
    THE_ONE_PROJECT_ROOT="\$(pwd)" THE_ONE_PROJECT_ID="demo" embedded-ui

${CYAN}${BOLD}POST-INSTALL SETUP (optional)${NC}
    All of these are optional — the MCP works with defaults.

    1. ${BOLD}Nano LLM routing${NC} — add Ollama/LiteLLM providers to config
       for smarter request classification (instead of keyword rules)

    2. ${BOLD}Remote Qdrant${NC} — connect a Qdrant server for persistent
       vector search (instead of in-memory keyword fallback)

    3. ${BOLD}API embeddings${NC} — use OpenAI/Voyage/Cohere embeddings
       for higher quality search (instead of local fastembed)

    4. ${BOLD}Tune limits${NC} — adjust max_search_hits, max_chunk_tokens,
       etc. in config.json for your token budget

EOF
}

# ═══════════════════════════════════════════════════════════════════════════
# ── MAIN ───────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)    VERSION="$2"; shift 2 ;;
        --version=*)  VERSION="${1#--version=}"; shift ;;
        --local)      LOCAL_DIR="$2"; shift 2 ;;
        --local=*)    LOCAL_DIR="${1#--local=}"; shift ;;
        --lean)       LEAN=true; shift ;;
        --uninstall)  UNINSTALL=true; shift ;;
        --skip-register)  SKIP_REGISTER=true; shift ;;
        --skip-validate)  SKIP_VALIDATE=true; shift ;;
        --yes|-y)     YES=true; shift ;;
        --no-color)   NO_COLOR=true; shift ;;
        --verbose)    VERBOSE=true; shift ;;
        --help|-h)    setup_colors; cmd_help; exit 0 ;;
        *)            err "Unknown option: $1"; echo "Run: install.sh --help"; exit 1 ;;
    esac
done

setup_colors

# ── Header ─────────────────────────────────────────────────────────────────
echo ""
echo "${CYAN}${BOLD}╔══════════════════════════════════════════════════╗${NC}"
echo "${CYAN}${BOLD}║  THE-ONE MCP — Installer                        ║${NC}"
echo "${CYAN}${BOLD}╚══════════════════════════════════════════════════╝${NC}"
echo ""

# ── Detect environment ─────────────────────────────────────────────────────
detect_platform
detect_clis

info "Platform: ${BOLD}${PLATFORM_NAME}${NC}"
info "Install dir: ${BOLD}${INSTALL_DIR}${NC}"
[ "$HAS_CLAUDE" = true ] && info "Claude Code: ${GREEN}${CLAUDE_VERSION}${NC}" || info "Claude Code: ${DIM}not found${NC}"
[ "$HAS_CODEX" = true ] && info "Codex: ${GREEN}${CODEX_VERSION}${NC}" || info "Codex: ${DIM}not found${NC}"
[ "$HAS_GEMINI" = true ] && info "Gemini CLI: ${GREEN}${GEMINI_VERSION}${NC}" || info "Gemini CLI: ${DIM}not found${NC}"
[ "$HAS_OPENCODE" = true ] && info "OpenCode: ${GREEN}${OPENCODE_VERSION}${NC}" || info "OpenCode: ${DIM}not found${NC}"
echo ""

# ── Uninstall path ─────────────────────────────────────────────────────────
if [ "$UNINSTALL" = true ]; then
    cmd_uninstall
    exit 0
fi

# ── Install ────────────────────────────────────────────────────────────────

# Step 1: Get binaries
if [ -n "$LOCAL_DIR" ]; then
    log "Step 1/6: Installing from local build (${LOCAL_DIR})"
    install_local
else
    resolve_version
    log "Step 1/6: Downloading ${VERSION} for ${PLATFORM_NAME}${LEAN:+ (lean)}"
    download_release
fi

# Step 2: Default config
log "Step 2/6: Setting up configuration"
mkdir -p "$INSTALL_DIR"
create_default_config

# Step 3: Embedding model selection
log "Step 3/6: Selecting embedding model"
select_embedding_model

# Step 4: Recommended tools
log "Step 4/6: Setting up tools catalog"
download_recommended_tools

# Step 5: PATH and CLI registration
log "Step 5/6: Registering with AI assistants"
ensure_path
register_all_clis

# Step 6: Validate
log "Step 6/6: Validating installation"
validate_install

# ── Summary ────────────────────────────────────────────────────────────────
echo ""
echo "${CYAN}${BOLD}══════════════════════════════════════════════════${NC}"
echo ""
log "${GREEN}${BOLD}Installation complete!${NC}"
echo ""
echo "  Binary:          ${BIN_DIR}/${BIN_NAME}${PLATFORM_EXT}"
echo "  Config:          ${CONFIG_FILE}"
echo "  Model:           $(grep -o '"embedding_model": "[^"]*"' "$CONFIG_FILE" | cut -d'"' -f4)"
echo "  Tools:           ${REGISTRY_DIR}/recommended.json"
echo "  Custom tools:    ${REGISTRY_DIR}/custom.json"
echo ""
echo "  ${BOLD}AI Assistant Status:${NC}"
if [ "$HAS_CLAUDE" = true ]; then
    echo "    ${GREEN}✓${NC} Claude Code (${CLAUDE_VERSION})"
else
    echo "    ${DIM}–${NC} Claude Code: claude mcp add ${BIN_NAME} -- ${BIN_DIR}/${BIN_NAME} serve"
fi
if [ "$HAS_GEMINI" = true ]; then
    echo "    ${GREEN}✓${NC} Gemini CLI (${GEMINI_VERSION})"
else
    echo "    ${DIM}–${NC} Gemini CLI:  gemini mcp add ${BIN_NAME} ${BIN_DIR}/${BIN_NAME} serve"
fi
if [ "$HAS_OPENCODE" = true ]; then
    echo "    ${GREEN}✓${NC} OpenCode (${OPENCODE_VERSION})"
else
    echo "    ${DIM}–${NC} OpenCode:    opencode mcp add --name ${BIN_NAME} --command ${BIN_DIR}/${BIN_NAME} --args serve"
fi
if [ "$HAS_CODEX" = true ]; then
    echo "    ${GREEN}✓${NC} Codex (${CODEX_VERSION})"
fi
echo ""
info "Customize: \$EDITOR ${CONFIG_FILE}"
info "Add tools: \$EDITOR ${REGISTRY_DIR}/custom.json"
info "Admin UI:  THE_ONE_PROJECT_ROOT=\"\$(pwd)\" ${BIN_DIR}/${UI_BIN_NAME}"
info "Uninstall: bash install.sh --uninstall"
echo ""
