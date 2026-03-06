"""Language detection based on file extensions."""

from __future__ import annotations

from pathlib import Path

SUPPORTED_EXTENSIONS: dict[str, str] = {
    ".py": "python",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".js": "javascript",
    ".jsx": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".ex": "elixir",
    ".exs": "elixir",
    ".rs": "rust",
    ".md": "markdown",
    ".go": "go",
    ".java": "java",
    ".yml": "yaml",
    ".yaml": "yaml",
    ".toml": "toml",
    ".sql": "sql",
    ".html": "html",
    ".css": "css",
    ".scss": "css",
    ".json": "json",
    ".csv": "csv",
    ".txt": "text",
}

def get_language(file_path: str | Path) -> str:
    """Return the language name for *file_path*. Fallback to "text"."""
    path = Path(file_path)
    # Check specific manifest files first
    if path.name == "Cargo.toml":
        return "toml"
    if path.name == "pyproject.toml":
        return "toml"
    if path.name == "package.json":
        return "json"
    if path.name == "mix.exs":
        return "elixir"

    suffix = path.suffix.lower()
    return SUPPORTED_EXTENSIONS.get(suffix, "text")

def is_supported(file_path: str | Path) -> bool:
    """Return ``True`` for any file (filtering happens at the I/O level)."""
    return True
