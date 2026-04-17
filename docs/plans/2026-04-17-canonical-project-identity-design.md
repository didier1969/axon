# Canonical Project Identity Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enforce one canonical project identity source across Axon ingestion so files are indexed only under registered projects and never under implicit fallback scopes.

**Architecture:** Keep the current scan filters and ingestion pipeline, but replace identity assignment with a registry-backed resolver built on `soll.ProjectCodeRegistry`. Per-project scans stay explicit when launched from a known registry entry; workspace-wide scans resolve by path. The worker becomes defensive and rejects unresolved files instead of defaulting to `global`.

**Tech Stack:** Rust, DuckDB/Canard via `GraphStore`, Axon scanner/watcher/worker pipeline.

---

## Decision Summary

- `.axon/meta.json` is canonical on disk.
- `soll.ProjectCodeRegistry` is canonical in runtime memory.
- Runtime memory is a mirror of file truth.
- `PRO` is reserved for SOLL global guidelines only.
- No implicit fallback is allowed for project import.

## Approach

### 1. Registry-backed resolution

Add resolution helpers that:

- read registered projects from `soll.ProjectCodeRegistry`
- resolve `project_code -> canonical identity`
- resolve `path -> canonical identity` by longest matching registered `project_path`

### 2. Scanner and watcher alignment

- Keep all existing eligibility filters.
- For workspace-wide scans, resolve each file path through the registry.
- If a file does not belong to a registered project, skip it instead of inventing a code.

### 3. Worker hardening

- Normalize parser output against canonical registry entries.
- If neither parser data nor file path resolves to a registered project, emit a skip event instead of producing `global`.

### 4. Verification

Targeted tests:

- registered path resolves to canonical code
- unknown path is rejected
- workspace-wide scan indexes only registered project files
- worker normalization no longer falls back to `global`
