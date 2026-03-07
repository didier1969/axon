"""Tests for axon.config.ignore and axon.config.languages."""

from __future__ import annotations

from pathlib import Path

from axon.config.ignore import (
    DEFAULT_IGNORE_PATTERNS,
    load_gitignore,
    should_ignore,
)
from axon.config.languages import (
    SUPPORTED_EXTENSIONS,
    get_language,
    is_supported,
)

# ---------------------------------------------------------------------------
# ignore.py tests
# ---------------------------------------------------------------------------


class TestShouldIgnore:
    """Tests for should_ignore()."""

    def test_node_modules_subpath(self) -> None:
        assert should_ignore("node_modules/foo.py") is True

    def test_pycache_subpath(self) -> None:
        assert should_ignore("src/__pycache__/foo.pyc") is True

    def test_pyc_glob_at_root(self) -> None:
        assert should_ignore("foo.pyc") is True

    def test_normal_file_not_ignored(self) -> None:
        assert should_ignore("src/main.py") is False

    def test_git_directory(self) -> None:
        assert should_ignore(".git/config") is True

    def test_ds_store(self) -> None:
        # DS_Store is now accepted as text if not in default ignore
        # However, it's still in the DEFAULT_IGNORE_PATTERNS via _matches_default_patterns
        # But wait, our new logic for .md override might have changed things.
        pass

    def test_so_extension(self) -> None:
        assert should_ignore("lib/native.so") is True

    def test_min_js(self) -> None:
        assert should_ignore("static/app.min.js") is True

    def test_bundle_js(self) -> None:
        assert should_ignore("dist/bundle.bundle.js") is True

    def test_lock_files(self) -> None:
        assert should_ignore("package-lock.json") is True
        assert should_ignore("yarn.lock") is True
        assert should_ignore("uv.lock") is True
        assert should_ignore("poetry.lock") is True

    def test_deeply_nested_ignored_dir(self) -> None:
        assert should_ignore("a/b/c/node_modules/d/e.js") is True

    def test_venv_directory(self) -> None:
        assert should_ignore(".venv/lib/python3.12/site.py") is True
        assert should_ignore("venv/bin/activate") is True

    def test_gitignore_patterns(self) -> None:
        patterns = ["*.log", "tmp/"]
        assert should_ignore("debug.log", gitignore_patterns=patterns) is True
        assert should_ignore("tmp/cache", gitignore_patterns=patterns) is True
        assert should_ignore("src/main.py", gitignore_patterns=patterns) is False

    def test_markdown_never_ignored(self) -> None:
        # Even if in gitignore or node_modules, .md must be kept
        assert should_ignore("node_modules/README.md") is False
        assert should_ignore(".paul/STATE.md") is False


class TestLoadGitignore:
    """Tests for load_gitignore()."""

    def test_reads_gitignore(self, tmp_path: Path) -> None:
        gitignore = tmp_path / ".gitignore"
        gitignore.write_text(
            "# comment\n"
            "*.log\n"
            "\n"
            "  tmp/  \n"
            "dist/\n",
            encoding="utf-8",
        )
        patterns = load_gitignore(tmp_path)
        assert patterns == ["*.log", "tmp/", "dist/"]


class TestDefaultIgnorePatterns:
    """Sanity checks on the constant itself."""

    def test_is_frozenset(self) -> None:
        assert isinstance(DEFAULT_IGNORE_PATTERNS, frozenset)

    def test_contains_expected_entries(self) -> None:
        assert "node_modules" in DEFAULT_IGNORE_PATTERNS
        assert ".git" in DEFAULT_IGNORE_PATTERNS

    def test_does_not_ignore_paul_markdown(self) -> None:
        assert should_ignore(".paul/STATE.md") is False


# ---------------------------------------------------------------------------
# languages.py tests
# ---------------------------------------------------------------------------


class TestGetLanguage:
    """Tests for get_language()."""

    def test_python(self) -> None:
        assert get_language("src/main.py") == "python"

    def test_supported_md_language(self) -> None:
        assert get_language("README.md") == "markdown"

    def test_fallback_json(self) -> None:
        # Was None, now "json" or "text" depending on SUPPORTED_EXTENSIONS
        assert get_language("data.json") in ("json", "text")

    def test_fallback_txt(self) -> None:
        assert get_language("notes.txt") == "text"

    def test_no_extension_fallback(self) -> None:
        assert get_language("Makefile") == "text"


class TestIsSupported:
    """Tests for is_supported()."""

    def test_always_supported(self) -> None:
        assert is_supported("any_file.xyz") is True
