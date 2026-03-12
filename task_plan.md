# Axon v2 - Tree-sitter Parsers Porting Plan

## Goal
Replace the legacy Python Tree-sitter extractors with native Rust implementations inside `axon-core/src/parser/` to achieve the sub-millisecond parsing latency required by the v2 architecture. We must retain 100% of the advanced extraction capabilities that made the Python parsers "the best on the planet" (e.g., OTP patterns in Elixir, NIF detection, etc.).

## Context
We have purged the legacy Python components (`src/axon/core/parsers/`). The new architecture expects files to be parsed natively within `axon-core` using `tree-sitter`. The core traits and structures are defined in `src/axon-core/src/parser/mod.rs`. We need to implement concrete parsers for all 11 languages previously supported.

For each parser, the implementer MUST reference the legacy Python code from the `origin/main` branch (using `git show origin/main:src/axon/core/parsers/<lang>.py`) to accurately port the advanced AST traversal logic.

## Tasks

### Task 1: Python Parser Implementation (COMPLETED)
- **File:** `src/axon-core/src/parser/python.rs`
- **Spec:** Implement the `Parser` trait for Python.
  - Port advanced Python logic from `origin/main:src/axon/core/parsers/python_lang.py`.
  - Extract: Classes, Functions/Methods, Imports, Function Calls.
- **Tests:** Add a unit test module within the file.

### Task 2: Advanced Elixir Parser Implementation (COMPLETED)
- **File:** `src/axon-core/src/parser/elixir.rs`
- **Spec:** Implement the `Parser` trait for Elixir.
  - Port advanced logic from `origin/main:src/axon/core/parsers/elixir_lang.py`.
  - Extract: Modules, Functions (`def`, `defp`), Macros (`defmacro`).
  - **Critical features:** OTP entry points (`handle_call`, `init`, etc.), NIF loaders, GenServer specific cross-process calls, and `@behaviour` extraction for heritage.
- **Tests:** Add a unit test module within the file.

### Task 3: TypeScript/JavaScript Parser Implementation
- **File:** `src/axon-core/src/parser/typescript.rs`
- **Spec:** Implement the `Parser` trait for TS/JS.
  - Port logic from `origin/main:src/axon/core/parsers/typescript.py`.
- **Tests:** Add a unit test module.

### Task 4: Rust Parser Implementation
- **File:** `src/axon-core/src/parser/rust.rs`
- **Spec:** Implement the `Parser` trait for Rust.
  - Port logic from `origin/main:src/axon/core/parsers/rust_lang.py`.
- **Tests:** Add a unit test module.

### Task 5: Go Parser Implementation
- **File:** `src/axon-core/src/parser/go.rs`
- **Spec:** Implement the `Parser` trait for Go.
  - Port logic from `origin/main:src/axon/core/parsers/go_lang.py`.
- **Tests:** Add a unit test module.

### Task 6: Java Parser Implementation
- **File:** `src/axon-core/src/parser/java.rs`
- **Spec:** Implement the `Parser` trait for Java.
  - Port logic from `origin/main:src/axon/core/parsers/java_lang.py`.
- **Tests:** Add a unit test module.

### Task 7: Web/Markup Parsers (HTML, CSS, Markdown)
- **Files:** `html.rs`, `css.rs`, `markdown.rs` inside `src/axon-core/src/parser/`.
- **Spec:** Port logic from their respective Python counterparts.
- **Tests:** Add unit tests.

### Task 8: Data/Config Parsers (SQL, YAML)
- **Files:** `sql.rs`, `yaml.rs` inside `src/axon-core/src/parser/`.
- **Spec:** Port logic from their respective Python counterparts.
- **Tests:** Add unit tests.

### Task 9: Parser Registry Integration
- **File:** `src/axon-core/src/parser/mod.rs`
- **Spec:** Update the `get_parser_for_file(path: &Path)` function to route to the correct parser based on file extension for all 11 languages.
- **Tests:** Add a unit test verifying routing works correctly for different extensions.