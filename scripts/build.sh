#!/usr/bin/env bash
set -euo pipefail

# ╔══════════════════════════════════════════════════════════════════════════╗
# ║  THE-ONE MCP — Build & Release Manager                                 ║
# ║  Production-ready build orchestration for the-one-mcp.                  ║
# ╚══════════════════════════════════════════════════════════════════════════╝
#
# ┌────────────────────────────────────────────────────────────────────────┐
# │  BUILD TARGETS                                                         │
# │                                                                        │
# │  the-one-mcp           MCP server binary (stdio/SSE/streamable HTTP)   │
# │  embedded-ui            Admin UI binary (dashboard on :8787)            │
# │                                                                        │
# │  FEATURE FLAGS                                                         │
# │                                                                        │
# │  embed-swagger (default: ON)                                           │
# │    ON  → OpenAPI/Swagger JSON baked into binary (~adds to size)        │
# │    OFF → Smaller binary, /swagger endpoint returns 404                 │
# │                                                                        │
# │  BUILD PROFILES                                                        │
# │                                                                        │
# │  release   Optimized, stripped   (default for: build, package)         │
# │  debug     Unoptimized, symbols  (default for: dev)                    │
# ├────────────────────────────────────────────────────────────────────────┤
# │  Key Commands                                                          │
# │                                                                        │
# │  build.sh build           Build release binary (with swagger)          │
# │  build.sh build --lean    Build release binary (no swagger)            │
# │  build.sh dev             Build debug binary (fast iteration)          │
# │  build.sh test            Run all workspace tests                      │
# │  build.sh check           Full CI pipeline: fmt + clippy + test        │
# │  build.sh package         Build + copy to dist/ with metadata          │
# │  build.sh clean           Remove build artifacts                       │
# │  build.sh install         Build + copy binary to ~/.local/bin/         │
# │  build.sh info            Show build configuration                     │
# ├────────────────────────────────────────────────────────────────────────┤
# │  Global Flags (place BEFORE the command)                               │
# │                                                                        │
# │  --dry-run     Show what would happen without doing it                 │
# │  --verbose     Show detailed output                                    │
# │  --no-color    Disable colored output                                  │
# │  --yes         Skip all confirmation prompts                           │
# └────────────────────────────────────────────────────────────────────────┘

# ── Resolve script location ────────────────────────────────────────────────
REAL_SCRIPT="${BASH_SOURCE[0]}"
while [ -L "$REAL_SCRIPT" ]; do
    REAL_DIR="$(cd "$(dirname "$REAL_SCRIPT")" && pwd)"
    REAL_SCRIPT="$(readlink "$REAL_SCRIPT")"
    [[ "$REAL_SCRIPT" != /* ]] && REAL_SCRIPT="$REAL_DIR/$REAL_SCRIPT"
done
readonly SCRIPT_DIR="$(cd "$(dirname "$REAL_SCRIPT")" && pwd)"
readonly PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

readonly VERSION="$(cat "$PROJECT_ROOT/VERSION" 2>/dev/null || echo "unknown")"
readonly BIN_NAME="the-one-mcp"
readonly UI_BIN_NAME="embedded-ui"
readonly MCP_CRATE="the-one-mcp"
readonly UI_CRATE="the-one-ui"

# ── Global Flags ───────────────────────────────────────────────────────────
DRY_RUN=false
VERBOSE=false
NO_COLOR=false
YES=false

# ── Colors (auto-disable for pipes and --no-color) ─────────────────────────
setup_colors() {
    if [ "$NO_COLOR" = true ]; then
        RED='' GREEN='' YELLOW='' BLUE='' CYAN='' DIM='' BOLD='' NC=''
    elif [ ! -t 1 ]; then
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

log()     { echo -e "${GREEN}[BUILD]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
err()     { echo -e "${RED}[ERROR]${NC} $*" >&2; }
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
debug()   { [ "$VERBOSE" = true ] && echo -e "${DIM}[DEBUG] $*${NC}" || true; }
dry()     { echo -e "${CYAN}[DRY-RUN]${NC} Would: $*"; }

# ── Build Parallelism ──────────────────────────────────────────────────────
resolve_build_jobs() {
    local configured="${BUILD_JOBS:-${CARGO_BUILD_JOBS:-}}"
    if [[ "$configured" =~ ^[0-9]+$ ]] && [ "$configured" -gt 0 ]; then
        BUILD_JOBS="$configured"
        BUILD_JOBS_SOURCE="env"
        return 0
    fi

    local cores=0
    if command -v nproc >/dev/null 2>&1; then
        cores=$(nproc 2>/dev/null || echo 0)
    elif command -v getconf >/dev/null 2>&1; then
        cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 0)
    elif command -v sysctl >/dev/null 2>&1; then
        cores=$(sysctl -n hw.ncpu 2>/dev/null || echo 0)
    fi

    if ! [[ "$cores" =~ ^[0-9]+$ ]] || [ "$cores" -le 0 ]; then
        cores=4
    fi

    local computed=$((cores - 2))
    if [ "$computed" -lt 1 ]; then
        computed=1
    fi
    BUILD_JOBS="$computed"
    BUILD_JOBS_SOURCE="auto(cores=$cores, reserve=2)"
    export CARGO_BUILD_JOBS="$BUILD_JOBS"
}

# ── Confirmation ───────────────────────────────────────────────────────────
confirm() {
    if [ "$YES" = true ]; then return 0; fi
    local prompt="${1:-Continue?}"
    echo -en "${YELLOW}${prompt} [y/N] ${NC}"
    read -r response
    [[ "$response" =~ ^[Yy] ]]
}

# ── Elapsed Time ───────────────────────────────────────────────────────────
format_elapsed() {
    local seconds=$1
    if [ "$seconds" -ge 60 ]; then
        printf "%dm%ds" $((seconds / 60)) $((seconds % 60))
    else
        printf "%ds" "$seconds"
    fi
}

# ── Binary Size ────────────────────────────────────────────────────────────
show_binary_info() {
    local bin_path="$1"
    local label="$2"
    if [ -f "$bin_path" ]; then
        local size
        size=$(du -h "$bin_path" | cut -f1)
        printf "  ${GREEN}%-24s${NC} %s\n" "$label" "$size"
    fi
}

# ═══════════════════════════════════════════════════════════════════════════
# ── BUILD ──────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_build() {
    local lean=false
    local skip_clippy=false
    local profile="release"
    local with_ui=false

    for arg in "$@"; do
        case "$arg" in
            --lean|--no-swagger)  lean=true ;;
            --skip-clippy)        skip_clippy=true ;;
            --debug)              profile="debug" ;;
            --with-ui)            with_ui=true ;;
            *) err "Unknown build option: $arg"; exit 1 ;;
        esac
    done

    resolve_build_jobs

    local feature_args=()
    local feature_label="with swagger"
    if [ "$lean" = true ]; then
        feature_args=(--no-default-features)
        feature_label="lean (no swagger)"
    fi

    local profile_flag=""
    [ "$profile" = "release" ] && profile_flag="--release"

    log "Building ${BIN_NAME} ${VERSION} (${profile}, ${feature_label}, jobs: ${BUILD_JOBS})"
    echo ""

    if [ "$DRY_RUN" = true ]; then
        dry "cargo build -p ${MCP_CRATE} --bin ${BIN_NAME} ${profile_flag} ${feature_args[*]:-}"
        [ "$skip_clippy" = false ] && dry "cargo clippy --workspace --all-targets -- -D warnings"
        [ "$with_ui" = true ] && dry "cargo build -p ${UI_CRATE} --bin ${UI_BIN_NAME} ${profile_flag}"
        return 0
    fi

    # Step 1: Build MCP binary
    local start_time=$SECONDS
    info "Step 1/$([ "$skip_clippy" = true ] && echo "1" || echo "2"): Compiling ${BIN_NAME}..."

    if cargo build -p "${MCP_CRATE}" --bin "${BIN_NAME}" ${profile_flag:+$profile_flag} "${feature_args[@]}" 2>&1; then
        local elapsed=$((SECONDS - start_time))
        log "Compiled ${BIN_NAME} ($(format_elapsed $elapsed))"
    else
        err "Build FAILED"
        exit 1
    fi

    # Step 1b: Build UI binary if requested
    if [ "$with_ui" = true ]; then
        info "Building ${UI_BIN_NAME}..."
        if cargo build -p "${UI_CRATE}" --bin "${UI_BIN_NAME}" ${profile_flag:+$profile_flag} 2>&1; then
            log "Compiled ${UI_BIN_NAME}"
        else
            err "UI build FAILED"
            exit 1
        fi
    fi

    # Step 2: Clippy
    if [ "$skip_clippy" = false ]; then
        info "Step 2/2: Running clippy..."
        if cargo clippy --workspace --all-targets -- -D warnings 2>&1; then
            log "Clippy clean"
        else
            warn "Clippy found issues"
            exit 1
        fi
    fi

    # Summary
    echo ""
    log "Build complete:"
    show_binary_info "target/${profile}/${BIN_NAME}" "${BIN_NAME}"
    [ "$with_ui" = true ] && show_binary_info "target/${profile}/${UI_BIN_NAME}" "${UI_BIN_NAME}"
    echo ""

    if [ "$lean" = true ]; then
        info "Built ${BOLD}without${NC} swagger. The /swagger endpoint will return 404."
    else
        info "Built ${BOLD}with${NC} swagger. OpenAPI UI available at /swagger."
    fi

    info "Binary: target/${profile}/${BIN_NAME}"
}

# ═══════════════════════════════════════════════════════════════════════════
# ── DEV ────────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_dev() {
    resolve_build_jobs

    log "Building workspace (debug, jobs: ${BUILD_JOBS})..."

    if [ "$DRY_RUN" = true ]; then
        dry "cargo build --workspace"
        return 0
    fi

    local start_time=$SECONDS
    if cargo build --workspace 2>&1; then
        local elapsed=$((SECONDS - start_time))
        log "Debug build complete ($(format_elapsed $elapsed))"
        echo ""
        show_binary_info "target/debug/${BIN_NAME}" "${BIN_NAME} (debug)"
        show_binary_info "target/debug/${UI_BIN_NAME}" "${UI_BIN_NAME} (debug)"
    else
        err "Build FAILED"
        exit 1
    fi
}

# ═══════════════════════════════════════════════════════════════════════════
# ── TEST ───────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_test() {
    local crate=""
    local test_name=""

    for arg in "$@"; do
        case "$arg" in
            -p|--package) : ;;  # next arg is the crate
            the-one-*) crate="$arg" ;;
            *) test_name="$arg" ;;
        esac
    done

    if [ "$DRY_RUN" = true ]; then
        if [ -n "$crate" ]; then
            dry "cargo test -p ${crate} ${test_name}"
        else
            dry "cargo test --workspace"
        fi
        return 0
    fi

    local start_time=$SECONDS

    if [ -n "$crate" ]; then
        log "Testing ${crate}${test_name:+ (filter: ${test_name})}..."
        cargo test -p "${crate}" ${test_name:+$test_name} 2>&1
    else
        log "Testing entire workspace..."
        cargo test --workspace 2>&1
    fi

    local elapsed=$((SECONDS - start_time))
    local total
    total=$(cargo test --workspace 2>&1 | grep "^test result: ok" | awk '{sum += $4} END {print sum}')
    log "Tests passed: ${total:-all} ($(format_elapsed $elapsed))"
}

# ═══════════════════════════════════════════════════════════════════════════
# ── CHECK (Full CI Pipeline) ──────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_check() {
    log "Running full CI pipeline for ${VERSION}..."
    echo ""

    if [ "$DRY_RUN" = true ]; then
        dry "cargo fmt --check"
        dry "cargo clippy --workspace --all-targets -- -D warnings"
        dry "cargo test --workspace"
        dry "cargo build --release -p ${MCP_CRATE} --bin ${BIN_NAME}"
        dry "bash scripts/release-gate.sh"
        return 0
    fi

    local start_time=$SECONDS
    local step=1

    # Step 1: Format
    info "Step ${step}/5: Checking formatting..."
    if cargo fmt --check 2>&1; then
        log "Format: OK"
    else
        err "Format check FAILED. Run: cargo fmt"
        exit 1
    fi
    step=$((step + 1))

    # Step 2: Clippy
    info "Step ${step}/5: Running clippy..."
    if cargo clippy --workspace --all-targets -- -D warnings 2>&1; then
        log "Clippy: clean"
    else
        err "Clippy FAILED"
        exit 1
    fi
    step=$((step + 1))

    # Step 3: Tests
    info "Step ${step}/5: Running tests..."
    if cargo test --workspace 2>&1; then
        log "Tests: passed"
    else
        err "Tests FAILED"
        exit 1
    fi
    step=$((step + 1))

    # Step 4: Release build
    info "Step ${step}/5: Building release binary..."
    if cargo build --release -p "${MCP_CRATE}" --bin "${BIN_NAME}" 2>&1; then
        log "Release build: OK"
    else
        err "Release build FAILED"
        exit 1
    fi
    step=$((step + 1))

    # Step 5: Release gate
    info "Step ${step}/5: Running release gate..."
    if bash scripts/release-gate.sh 2>&1; then
        log "Release gate: passed"
    else
        err "Release gate FAILED"
        exit 1
    fi

    local elapsed=$((SECONDS - start_time))
    echo ""
    log "${GREEN}${BOLD}All checks passed${NC} ($(format_elapsed $elapsed))"
    show_binary_info "target/release/${BIN_NAME}" "${BIN_NAME}"
}

# ═══════════════════════════════════════════════════════════════════════════
# ── PACKAGE ────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_package() {
    local lean=false
    local with_ui=false

    for arg in "$@"; do
        case "$arg" in
            --lean|--no-swagger) lean=true ;;
            --with-ui)           with_ui=true ;;
            *) err "Unknown package option: $arg"; exit 1 ;;
        esac
    done

    local dist_dir="${PROJECT_ROOT}/dist"
    local pkg_name="${BIN_NAME}-${VERSION}"
    local pkg_dir="${dist_dir}/${pkg_name}"

    log "Packaging ${BIN_NAME} ${VERSION}..."

    # Build first
    local build_args=()
    [ "$lean" = true ] && build_args+=(--lean)
    [ "$with_ui" = true ] && build_args+=(--with-ui)
    build_args+=(--skip-clippy)
    cmd_build "${build_args[@]}"

    if [ "$DRY_RUN" = true ]; then
        dry "mkdir -p ${pkg_dir}"
        dry "cp target/release/${BIN_NAME} ${pkg_dir}/"
        dry "write ${pkg_dir}/BUILD_INFO"
        return 0
    fi

    # Create package directory
    rm -rf "${pkg_dir}"
    mkdir -p "${pkg_dir}"

    # Copy binaries
    cp "target/release/${BIN_NAME}" "${pkg_dir}/"
    [ "$with_ui" = true ] && [ -f "target/release/${UI_BIN_NAME}" ] && \
        cp "target/release/${UI_BIN_NAME}" "${pkg_dir}/"

    # Write build metadata
    cat > "${pkg_dir}/BUILD_INFO" <<EOF
version=${VERSION}
build_date=$(date -u +%Y-%m-%dT%H:%M:%SZ)
git_commit=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
git_branch=$(git branch --show-current 2>/dev/null || echo "unknown")
rustc_version=$(rustc --version 2>/dev/null || echo "unknown")
features=$([ "$lean" = true ] && echo "lean" || echo "embed-swagger")
os=$(uname -s)
arch=$(uname -m)
EOF

    # Copy schemas
    if [ -d "schemas" ]; then
        cp -r schemas "${pkg_dir}/"
    fi

    echo ""
    log "Package created: ${pkg_dir}/"
    ls -lh "${pkg_dir}/"
    echo ""

    # Offer to create tarball
    if confirm "Create tarball?"; then
        local tarball="${dist_dir}/${pkg_name}.tar.gz"
        tar -czf "${tarball}" -C "${dist_dir}" "${pkg_name}"
        local tarball_size
        tarball_size=$(du -h "${tarball}" | cut -f1)
        log "Tarball: ${tarball} (${tarball_size})"
    fi
}

# ═══════════════════════════════════════════════════════════════════════════
# ── INSTALL ────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_install() {
    local lean=false
    local install_dir="${HOME}/.local/bin"

    for arg in "$@"; do
        case "$arg" in
            --lean|--no-swagger) lean=true ;;
            --dir=*)             install_dir="${arg#--dir=}" ;;
            *) err "Unknown install option: $arg"; exit 1 ;;
        esac
    done

    log "Installing ${BIN_NAME} ${VERSION} to ${install_dir}/"

    # Build first
    local build_args=(--skip-clippy)
    [ "$lean" = true ] && build_args+=(--lean)
    cmd_build "${build_args[@]}"

    if [ "$DRY_RUN" = true ]; then
        dry "mkdir -p ${install_dir}"
        dry "cp target/release/${BIN_NAME} ${install_dir}/"
        return 0
    fi

    mkdir -p "${install_dir}"
    cp "target/release/${BIN_NAME}" "${install_dir}/"
    chmod +x "${install_dir}/${BIN_NAME}"

    echo ""
    log "Installed: ${install_dir}/${BIN_NAME}"
    info "Make sure ${install_dir} is in your PATH"
    echo ""
    info "Usage:"
    echo "  ${BIN_NAME} serve                              # stdio (Claude Code)"
    echo "  ${BIN_NAME} serve --transport sse --port 3000   # SSE"
    echo "  ${BIN_NAME} serve --transport stream --port 3000 # streamable HTTP"
    echo ""
    info "Add to Claude Code:"
    echo "  claude mcp add ${BIN_NAME} -- ${install_dir}/${BIN_NAME} serve"
}

# ═══════════════════════════════════════════════════════════════════════════
# ── CLEAN ──────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_clean() {
    local clean_build=false
    local clean_dist=false
    local clean_cache=false
    local clean_all=false

    if [ $# -eq 0 ]; then
        clean_build=true
    fi

    for arg in "$@"; do
        case "$arg" in
            --build)   clean_build=true ;;
            --dist)    clean_dist=true ;;
            --cache)   clean_cache=true ;;
            --all)     clean_all=true ;;
            *) err "Unknown clean option: $arg"; exit 1 ;;
        esac
    done

    if [ "$clean_all" = true ]; then
        clean_build=true
        clean_dist=true
        clean_cache=true
    fi

    if [ "$DRY_RUN" = true ]; then
        [ "$clean_build" = true ] && dry "cargo clean"
        [ "$clean_dist" = true ] && dry "rm -rf dist/"
        [ "$clean_cache" = true ] && dry "rm -rf .fastembed_cache/"
        return 0
    fi

    if [ "$clean_build" = true ]; then
        log "Cleaning build artifacts..."
        cargo clean 2>&1
        log "Build artifacts removed"
    fi

    if [ "$clean_dist" = true ]; then
        if [ -d "dist" ]; then
            log "Cleaning dist/"
            rm -rf dist/
            log "dist/ removed"
        fi
    fi

    if [ "$clean_cache" = true ]; then
        if [ -d ".fastembed_cache" ]; then
            log "Cleaning fastembed model cache..."
            rm -rf .fastembed_cache/
            log "Model cache removed (will re-download on next use)"
        fi
    fi
}

# ═══════════════════════════════════════════════════════════════════════════
# ── INFO ───────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_info() {
    resolve_build_jobs

    echo ""
    echo "${CYAN}${BOLD}THE-ONE MCP Build Configuration${NC}"
    echo ""
    printf "  %-24s %s\n" "Version:" "${VERSION}"
    printf "  %-24s %s\n" "Binary:" "${BIN_NAME}"
    printf "  %-24s %s\n" "UI Binary:" "${UI_BIN_NAME}"
    printf "  %-24s %s\n" "Project Root:" "${PROJECT_ROOT}"
    printf "  %-24s %s\n" "Rust:" "$(rustc --version 2>/dev/null || echo 'not found')"
    printf "  %-24s %s\n" "Cargo:" "$(cargo --version 2>/dev/null || echo 'not found')"
    printf "  %-24s %s\n" "Build Jobs:" "${BUILD_JOBS} (${BUILD_JOBS_SOURCE})"
    printf "  %-24s %s\n" "Default Features:" "embed-swagger"
    echo ""

    echo "${CYAN}${BOLD}Workspace Crates${NC}"
    echo ""
    for toml in crates/*/Cargo.toml; do
        local crate_name
        crate_name=$(grep "^name" "$toml" | head -1 | sed 's/.*= *"\(.*\)"/\1/')
        printf "  %-24s %s\n" "${crate_name}" "$(dirname "$toml")"
    done
    echo ""

    # Show existing binaries
    echo "${CYAN}${BOLD}Built Binaries${NC}"
    echo ""
    if [ -f "target/release/${BIN_NAME}" ]; then
        show_binary_info "target/release/${BIN_NAME}" "${BIN_NAME} (release)"
    fi
    if [ -f "target/debug/${BIN_NAME}" ]; then
        show_binary_info "target/debug/${BIN_NAME}" "${BIN_NAME} (debug)"
    fi
    if [ -f "target/release/${UI_BIN_NAME}" ]; then
        show_binary_info "target/release/${UI_BIN_NAME}" "${UI_BIN_NAME} (release)"
    fi
    if [ ! -f "target/release/${BIN_NAME}" ] && [ ! -f "target/debug/${BIN_NAME}" ]; then
        info "No binaries built yet. Run: build.sh build"
    fi
    echo ""

    # Schema count
    local schema_count
    schema_count=$(find schemas/mcp/v1beta -name "*.json" 2>/dev/null | wc -l)
    printf "  %-24s %s\n" "v1beta Schemas:" "${schema_count} files"
    echo ""
}

# ═══════════════════════════════════════════════════════════════════════════
# ── HELP ───────────────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

cmd_help() {
    cat <<EOF
${CYAN}${BOLD}THE-ONE MCP — Build & Release Manager ${VERSION}${NC}

${CYAN}${BOLD}USAGE${NC}
    build.sh [global-flags] <command> [options]
    build.sh help <command>

${CYAN}${BOLD}GLOBAL FLAGS${NC}
    --dry-run       Show what would happen without doing it
    --verbose       Show detailed debug output
    --no-color      Disable colored output (auto-disabled when piping)
    --yes           Skip all confirmation prompts

${CYAN}${BOLD}COMMANDS${NC}
  ${GREEN}Build${NC}
    build           Build release binary (default: with swagger)
    dev             Build debug workspace (fast iteration)
    check           Full CI pipeline: fmt → clippy → test → build → release-gate
    test            Run workspace tests (or specific crate/test)

  ${GREEN}Distribution${NC}
    package         Build + create dist/ package with metadata
    install         Build + copy binary to ~/.local/bin/

  ${GREEN}Maintenance${NC}
    clean           Remove build artifacts
    info            Show build configuration and binary sizes
    help            This help message

${CYAN}${BOLD}BUILD OPTIONS${NC}
    --lean            Build without swagger (smaller binary, /swagger returns 404)
    --no-swagger      Alias for --lean
    --skip-clippy     Skip clippy check after build
    --debug           Build in debug profile instead of release
    --with-ui         Also build the embedded-ui binary

${CYAN}${BOLD}TEST OPTIONS${NC}
    <crate>           Test specific crate (e.g., the-one-core)
    <test-name>       Filter to specific test name

${CYAN}${BOLD}CLEAN OPTIONS${NC}
    --build           Remove target/ (default if no flags)
    --dist            Remove dist/
    --cache           Remove .fastembed_cache/ (model re-downloads on next use)
    --all             Remove everything

${CYAN}${BOLD}INSTALL OPTIONS${NC}
    --lean            Install without swagger
    --dir=<path>      Custom install directory (default: ~/.local/bin)

${CYAN}${BOLD}PACKAGE OPTIONS${NC}
    --lean            Package without swagger
    --with-ui         Include embedded-ui binary in package

${CYAN}${BOLD}EXAMPLES${NC}
    ${GREEN}# Standard release build (with swagger):${NC}
    build.sh build

    ${GREEN}# Lean build (no swagger, smaller binary):${NC}
    build.sh build --lean

    ${GREEN}# Build everything including admin UI:${NC}
    build.sh build --with-ui

    ${GREEN}# Quick debug build for development:${NC}
    build.sh dev

    ${GREEN}# Run all tests:${NC}
    build.sh test

    ${GREEN}# Test a specific crate:${NC}
    build.sh test the-one-memory

    ${GREEN}# Test a specific test by name:${NC}
    build.sh test the-one-core test_create_and_get

    ${GREEN}# Full CI validation (what CI runs):${NC}
    build.sh check

    ${GREEN}# Install to default location:${NC}
    build.sh install

    ${GREEN}# Install lean version to custom path:${NC}
    build.sh install --lean --dir=/usr/local/bin

    ${GREEN}# Create distributable package:${NC}
    build.sh package

    ${GREEN}# Create lean package with UI:${NC}
    build.sh package --lean --with-ui

    ${GREEN}# Preview what build would do:${NC}
    build.sh --dry-run build

    ${GREEN}# Clean everything:${NC}
    build.sh clean --all

    ${GREEN}# Show build info:${NC}
    build.sh info

${CYAN}${BOLD}FEATURE FLAGS${NC}
    embed-swagger     Bakes OpenAPI/Swagger JSON into the binary.
                      ON by default. Use --lean to disable.

                      With swagger:    /swagger serves interactive Swagger UI
                                       /api/swagger serves raw OpenAPI JSON

                      Without swagger: /swagger returns 404
                                       Binary is smaller

${CYAN}${BOLD}AFTER BUILDING${NC}
    ${GREEN}# Run as MCP server (stdio, for Claude Code):${NC}
    ./target/release/${BIN_NAME} serve

    ${GREEN}# Add to Claude Code:${NC}
    claude mcp add ${BIN_NAME} -- \$(pwd)/target/release/${BIN_NAME} serve

    ${GREEN}# Run with SSE transport:${NC}
    ./target/release/${BIN_NAME} serve --transport sse --port 3000

    ${GREEN}# Run with streamable HTTP transport:${NC}
    ./target/release/${BIN_NAME} serve --transport stream --port 3000

    ${GREEN}# Run admin UI:${NC}
    THE_ONE_PROJECT_ROOT="\$(pwd)" THE_ONE_PROJECT_ID="demo" \\
        ./target/release/${UI_BIN_NAME}

${CYAN}${BOLD}ENVIRONMENT${NC}
    BUILD_JOBS        Override build parallelism (default: auto)
    CARGO_BUILD_JOBS  Alternative to BUILD_JOBS
    RUST_LOG          Log level for the MCP server (default: info)

EOF
}

# ── Subcommand Help ────────────────────────────────────────────────────────
cmd_subcommand_help() {
    local command="${1:-}"

    case "$command" in
        build)
            cat <<EOF
Usage: build.sh build [options]

Build the ${BIN_NAME} release binary.

Options:
  --lean, --no-swagger    Build without embedded swagger (smaller binary)
  --skip-clippy           Skip clippy validation after build
  --debug                 Build with debug profile instead of release
  --with-ui               Also build the embedded-ui binary

Default features: embed-swagger (ON)

Examples:
  build.sh build                  Standard release build with swagger
  build.sh build --lean           Lean build without swagger
  build.sh build --with-ui        Build MCP server + admin UI
  build.sh build --debug          Debug build (faster, unoptimized)
EOF
            ;;
        test)
            cat <<EOF
Usage: build.sh test [crate] [test-name]

Run workspace tests, or filter to specific crate/test.

Examples:
  build.sh test                             All workspace tests
  build.sh test the-one-core                All tests in the-one-core crate
  build.sh test the-one-memory chunker      Only chunker tests in the-one-memory
  build.sh test the-one-mcp test_dispatch   Tests matching "test_dispatch"
EOF
            ;;
        check)
            cat <<EOF
Usage: build.sh check

Run the full CI validation pipeline:
  1. cargo fmt --check
  2. cargo clippy --workspace --all-targets -- -D warnings
  3. cargo test --workspace
  4. cargo build --release -p ${MCP_CRATE} --bin ${BIN_NAME}
  5. bash scripts/release-gate.sh

This is equivalent to what CI runs on every push/PR.
EOF
            ;;
        clean)
            cat <<EOF
Usage: build.sh clean [options]

Remove build artifacts.

Options:
  --build     Remove target/ directory (default if no flags)
  --dist      Remove dist/ directory
  --cache     Remove .fastembed_cache/ (model will re-download on next use)
  --all       Remove everything
EOF
            ;;
        install)
            cat <<EOF
Usage: build.sh install [options]

Build release binary and copy to install directory.

Options:
  --lean, --no-swagger  Build without embedded swagger
  --dir=<path>          Install directory (default: ~/.local/bin)

After install, add to Claude Code:
  claude mcp add ${BIN_NAME} -- ~/.local/bin/${BIN_NAME} serve
EOF
            ;;
        package)
            cat <<EOF
Usage: build.sh package [options]

Build and create a distributable package in dist/.

Options:
  --lean, --no-swagger  Package without swagger
  --with-ui             Include embedded-ui binary

Creates:
  dist/${BIN_NAME}-{version}/
    ${BIN_NAME}          MCP server binary
    ${UI_BIN_NAME}       Admin UI binary (if --with-ui)
    BUILD_INFO           Version, commit, date, features
    schemas/             v1beta JSON schemas
EOF
            ;;
        *)
            err "No help available for: ${command}"
            info "Available commands: build, dev, test, check, package, install, clean, info, help"
            return 1
            ;;
    esac
}

# ═══════════════════════════════════════════════════════════════════════════
# ── MAIN DISPATCH ──────────────────────────────────────────────────────────
# ═══════════════════════════════════════════════════════════════════════════

# Parse global flags
while [[ "${1:-}" == --* ]]; do
    case "$1" in
        --dry-run)   DRY_RUN=true; shift ;;
        --verbose)   VERBOSE=true; shift ;;
        --no-color)  NO_COLOR=true; shift ;;
        --yes)       YES=true; shift ;;
        --help|-h)   setup_colors; cmd_help; exit 0 ;;
        *)           break ;;
    esac
done

setup_colors

if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — no changes will be made"
    echo ""
fi

COMMAND="${1:-help}"
shift || true

# Handle --help after command
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cmd_subcommand_help "$COMMAND"
    exit $?
fi

case "$COMMAND" in
    build)    cmd_build "$@" ;;
    dev)      cmd_dev "$@" ;;
    test)     cmd_test "$@" ;;
    check)    cmd_check "$@" ;;
    package)  cmd_package "$@" ;;
    install)  cmd_install "$@" ;;
    clean)    cmd_clean "$@" ;;
    info)     cmd_info "$@" ;;
    help|--help|-h)
        if [ $# -gt 0 ]; then
            cmd_subcommand_help "$1"
        else
            cmd_help
        fi
        ;;
    *)
        err "Unknown command: ${COMMAND}"
        info "Run 'build.sh help' for usage"
        exit 1
        ;;
esac
