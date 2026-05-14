# Axon v2 - Tree-sitter Parsers Porting Plan

## Goal
Replace the legacy Python Tree-sitter extractors with native Rust implementations inside `axon-core/src/parser/` to achieve the sub-millisecond parsing latency required by the v2 architecture.

## Context
We have purged the legacy Python components (`src/axon/core/parsers/`). The new architecture expects files to be parsed natively within `axon-core` using `tree-sitter`. The core traits and structures are defined in `src/axon-core/src/parser/mod.rs`. We need to implement concrete parsers for Python, Elixir, and TypeScript.

## Tasks

### Task 1: Python Parser Implementation
- **File:** `src/axon-core/src/parser/python.rs`
- **Spec:** Implement the `Parser` trait for Python.
  - Use `tree-sitter-python` to parse the file content.
  - Define Scheme queries to extract:
    - **Classes** (label: `class`)
    - **Functions/Methods** (label: `function`)
    - **Imports** (relations `IMPORTS`)
    - **Function Calls** (relations `CALLS`)
  - The implementation must return a `crate::parser::ExtractionResult` populated with `Symbol` and `Relation` structs.
- **Tests:** Add a unit test module within the file that parses a basic python string and verifies symbols and relations.

### Task 2: Elixir Parser Implementation
- **File:** `src/axon-core/src/parser/elixir.rs`
- **Spec:** Implement the `Parser` trait for Elixir.
  - Use `tree-sitter-elixir` to parse the file content.
  - Define Scheme queries to extract:
    - **Modules** (`defmodule` -> label: `module`)
    - **Functions** (`def`, `defp`, `defmacro` -> label: `function`)
    - **Function Calls** (relations `CALLS`)
  - Return an `ExtractionResult`.
- **Tests:** Add a unit test module within the file verifying module and function extraction.

### Task 3: TypeScript Parser Implementation
- **File:** `src/axon-core/src/parser/typescript.rs`
- **Spec:** Implement the `Parser` trait for TypeScript.
  - Use `tree-sitter-typescript` to parse the file content.
  - Define Scheme queries to extract:
    - **Classes / Interfaces** (label: `class` / `interface`)
    - **Functions / Methods** (label: `function`)
    - **Function Calls** (relations `CALLS`)
  - Return an `ExtractionResult`.
- **Tests:** Add a unit test module within the file verifying class, interface, and function extraction.

### Task 4: Parser Registry Integration
- **File:** `src/axon-core/src/parser/mod.rs`
- **Spec:** Update the `get_parser_for_file(path: &Path)` function to route to the correct parser based on file extension.
  - `.py` -> `python::PythonParser`
  - `.ex`, `.exs` -> `elixir::ElixirParser`
  - `.ts`, `.tsx` -> `typescript::TypeScriptParser`
- **Tests:** Add a unit test verifying routing works correctly for different extensions.