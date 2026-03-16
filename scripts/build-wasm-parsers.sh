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

echo "Building WASM parsers in $PARSERS_DIR..."

cd "$TMP_DIR"

for lang in "${!GRAMMARS[@]}"; do
    repo="${GRAMMARS[$lang]}"
    echo "Processing $lang from $repo..."
    
    # Clone the grammar repository
    git clone --depth 1 "$repo" "tree-sitter-$lang"
    
    if [ "$lang" == "typescript" ]; then
        # Build typescript and tsx
        tree-sitter build --wasm "tree-sitter-$lang/typescript"
        mv tree-sitter-typescript.wasm "$PARSERS_DIR/" || true
        
        tree-sitter build --wasm "tree-sitter-$lang/tsx"
        mv tree-sitter-tsx.wasm "$PARSERS_DIR/" || true
    elif [ "$lang" == "markdown" ]; then
        tree-sitter build --wasm "tree-sitter-$lang/tree-sitter-markdown"
        mv tree-sitter-markdown.wasm "$PARSERS_DIR/" || true
        
        tree-sitter build --wasm "tree-sitter-$lang/tree-sitter-markdown-inline"
        mv tree-sitter-markdown-inline.wasm "$PARSERS_DIR/" || true
    else
        tree-sitter build --wasm "tree-sitter-$lang"
        mv "tree-sitter-${lang}.wasm" "$PARSERS_DIR/" || true
    fi
done

echo "Successfully built WASM parsers in $PARSERS_DIR"
