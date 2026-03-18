#!/usr/bin/env bash
set -e

# Define directories
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARSERS_DIR="${ROOT_DIR}/src/axon-core/parsers"
TMP_DIR="$(mktemp -d)"

# Ensure parsers directory exists
mkdir -p "$PARSERS_DIR"

# Clean up temp dir on exit
trap 'rm -rf "$TMP_DIR"' EXIT

# Grammars to build and their repository URLs
declare -A GRAMMARS=(
    ["python"]="https://github.com/tree-sitter/tree-sitter-python"
    ["elixir"]="https://github.com/elixir-lang/tree-sitter-elixir"
    ["rust"]="https://github.com/tree-sitter/tree-sitter-rust"
    ["typescript"]="https://github.com/tree-sitter/tree-sitter-typescript"
    ["javascript"]="https://github.com/tree-sitter/tree-sitter-javascript"
    ["go"]="https://github.com/tree-sitter/tree-sitter-go"
    ["java"]="https://github.com/tree-sitter/tree-sitter-java"
    ["html"]="https://github.com/tree-sitter/tree-sitter-html"
    ["css"]="https://github.com/tree-sitter/tree-sitter-css"
    ["markdown"]="https://github.com/tree-sitter-grammars/tree-sitter-markdown"
    ["yaml"]="https://github.com/tree-sitter-grammars/tree-sitter-yaml"
)

# ABI 14 compatible tags
declare -A TAGS=(
    ["python"]="v0.21.0"
    ["elixir"]="v0.3.0"
    ["rust"]="v0.21.2"
    ["typescript"]="v0.21.2"
    ["javascript"]="v0.21.2"
    ["go"]="v0.21.2"
    ["java"]="v0.21.0"
    ["html"]="v0.20.0"
    ["css"]="v0.21.0"
    ["markdown"]="v0.2.1"
    ["yaml"]="v0.6.1"
)

echo "Building WASM parsers in $PARSERS_DIR..."

cd "$TMP_DIR"

for lang in "${!GRAMMARS[@]}"; do
    repo="${GRAMMARS[$lang]}"
    tag="${TAGS[$lang]}"
    echo "Processing $lang from $repo @ $tag..."
    
    # Clone the grammar repository at the specific tag
    git clone --depth 1 --branch "$tag" "$repo" "tree-sitter-$lang"
    
    if [ "$lang" == "typescript" ]; then
        # Build typescript and tsx
        cd "tree-sitter-$lang/typescript" && tree-sitter build --wasm && mv tree-sitter-typescript.wasm "$PARSERS_DIR/" || true
        cd "$TMP_DIR"
        
        cd "tree-sitter-$lang/tsx" && tree-sitter build --wasm && mv tree-sitter-tsx.wasm "$PARSERS_DIR/" || true
        cd "$TMP_DIR"
    elif [ "$lang" == "markdown" ]; then
        cd "tree-sitter-$lang/tree-sitter-markdown" && tree-sitter build --wasm && mv tree-sitter-markdown.wasm "$PARSERS_DIR/" || true
        cd "$TMP_DIR"
        
        cd "tree-sitter-$lang/tree-sitter-markdown-inline" && tree-sitter build --wasm && mv tree-sitter-markdown-inline.wasm "$PARSERS_DIR/" || true
        cd "$TMP_DIR"
    else
        cd "tree-sitter-$lang" && tree-sitter build --wasm && mv "tree-sitter-${lang}.wasm" "$PARSERS_DIR/" || true
        cd "$TMP_DIR"
    fi
done

echo "Successfully built WASM parsers in $PARSERS_DIR"
