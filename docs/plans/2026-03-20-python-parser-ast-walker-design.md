# Python Parser AST Walker Redesign (v3.0)

## The Objective
To replace the naive, query-based Python parser with a full, recursive Abstract Syntax Tree (AST) Walker. This will enable the extraction of `CALLS` and `IMPORTS` relations, unlocking the full potential of Axon's Semantic Intelligence (Taint Analysis and exact Test Coverage) for Python projects.

## The Approach: AST Walker
We will implement the `walk` methodology currently used in our highly successful Rust and Go parsers.

### Core Mechanisms
1. **Scope Tracking:** The `walk` function will pass a `current_scope` string down the recursion tree. When traversing inside a `class_definition`, the scope becomes the class name. When inside a `function_definition`, the scope becomes `class_name.function_name`.
2. **Call Extraction (`call`):** When encountering a `call` node, the parser will extract the target function name and create a `Relation { from: current_scope, to: target_name, rel_type: "calls" }`.
3. **Import Extraction (`import_statement`, `import_from_statement`):** Will extract modules and create `Relation { from: module, to: imported_module, rel_type: "imports" }`.

### Taint Analysis Support
By extracting `call` relationships, any invocation of dangerous built-ins like `eval`, `exec`, or `subprocess.run` will become a node in the graph, finally allowing the Cypher Taint Analysis queries in KuzuDB to find valid paths and calculate accurate Security Scores.

## Implementation Steps
1. Refactor `src/axon-core/src/parser/python.rs`.
2. Remove `tree_sitter::QueryCursor`.
3. Implement `walk`, `extract_class`, `extract_function`, `extract_call`, and `extract_import` logic.
4. Update unit tests to verify relation extraction.