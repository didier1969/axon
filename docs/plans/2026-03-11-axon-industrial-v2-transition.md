# Axon v2 : Transition Industrielle (Phase 1) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create the standalone Rust Data Plane (`axon-core`) that integrates file scanning and high-performance Tree-sitter parsing.

**Architecture:** A unified Rust binary leveraging `tokio` for async orchestration, `ignore` for scanning, and `tree-sitter` for parsing.

**Tech Stack:** Rust 1.80+, Tree-sitter, Rayon, Serde (MsgPack).

---

### Task 1: Initialize Rust Workspace

**Files:**
- Create: `src/axon-core/Cargo.toml`
- Create: `src/axon-core/src/main.rs`

**Step 1: Create Cargo.toml**
```toml
[package]
name = "axon-core"
version = "2.0.0"
edition = "2021"

[dependencies]
tokio = { version = "1.36", features = ["full"] }
ignore = "0.4"
tree-sitter = "0.20"
serde = { version = "1.0", features = ["derive"] }
rmp-serde = "1.1"
anyhow = "1.0"
rayon = "1.8"
```

**Step 2: Run init test**
Run: `cd src/axon-core && cargo build`
Expected: Success

**Step 3: Commit**
```bash
git add src/axon-core/
git commit -m "infra: initialize axon-core rust workspace"
```

---

### Task 2: Implement High-Speed Scanner

**Files:**
- Create: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/main.rs`

**Step 1: Write scanner logic**
- Use `ignore::WalkBuilder`.
- Filter for supported extensions (.py, .ex, .exs, .ts, .rs).

**Step 2: Write unit test in scanner.rs**
- Mock a directory structure.
- Verify file count and filtering.

**Step 3: Commit**
```bash
git commit -m "feat: implement high-speed file scanner in rust"
```

---

### Task 3: Integrate Tree-sitter Engines

**Files:**
- Modify: `src/axon-core/Cargo.toml`
- Create: `src/axon-core/src/parser/mod.rs`
- Create: `src/axon-core/src/parser/python.rs`

**Step 1: Add language dependencies**
```toml
tree-sitter-python = "0.20"
tree-sitter-elixir = "0.2"
```

**Step 2: Implement Base Parser trait**
- Define `parse(content: &str) -> ExtractionResult`.

**Step 3: Port Python extraction logic**
- Implement `extract_symbols` using Tree-sitter queries.
- Port the queries from `src/axon/core/parsers/python_lang.py`.

**Step 4: Commit**
```bash
git commit -m "feat: port python tree-sitter extraction to rust"
```

---

### Task 4: Parallel Processing Pipeline

**Files:**
- Modify: `src/axon-core/src/main.rs`

**Step 1: Orchestrate Scan -> Parse**
- Use `Rayon` to process files in parallel.
- Print summary: "Parsed X files in Y ms".

**Step 2: Validation**
- Run on `axon` project itself.
- Verify that it extracts a known number of Python symbols.

**Step 3: Commit**
```bash
git commit -m "feat: implement parallel parsing pipeline"
```
