#!/bin/bash
set -euo pipefail

PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"

if ! command -v devenv >/dev/null 2>&1; then
  echo "❌ devenv n'est pas disponible dans le PATH."
  exit 1
fi

echo "🔎 Axon DevEnv validation"
echo "Project: $PROJECT_ROOT"
echo "devenv: $(devenv --version)"

required_env=(
  MIX_HOME
  HEX_HOME
  CARGO_TARGET_DIR
  RELEASE_COOKIE
  PHX_PORT
  HYDRA_HTTP_PORT
  LIBCLANG_PATH
  PYTHONPATH
)

echo ""
echo "Environment variables:"
missing_env=0
for key in "${required_env[@]}"; do
  value="${!key:-}"
  if [ -z "$value" ]; then
    echo "  MISSING $key"
    missing_env=1
  else
    echo "  OK      $key=$value"
  fi
done

echo ""
echo "Toolchain origins:"
required_tools=(python uv cargo rustc mix elixir tmux nc curl)
missing_tools=0
for tool in "${required_tools[@]}"; do
  path="$(command -v "$tool" || true)"
  if [ -z "$path" ]; then
    echo "  MISSING $tool"
    missing_tools=1
    continue
  fi

  case "$path" in
    /nix/store/*|/home/dstadel/projects/axon/.devenv/*)
      origin="devenv"
      ;;
    *)
      origin="external"
      ;;
  esac

  echo "  $tool -> $path [$origin]"
done

echo ""
if [ "$missing_env" -ne 0 ] || [ "$missing_tools" -ne 0 ]; then
  echo "❌ L'environnement courant n'est pas un shell Devenv valide."
  echo "   Utilise: devenv shell"
  exit 1
fi

echo "✅ Les variables et outils principaux sont présents."
