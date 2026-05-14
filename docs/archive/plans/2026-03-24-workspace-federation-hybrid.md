# Workspace Federation (Hybrid Approach) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract local path dependencies from Elixir (`mix.exs`), Python (`pyproject.toml`), and Rust (`Cargo.toml`) projects to build a unified cross-project dependency graph (`DEPENDS_ON` relations).

**Architecture:** Scenario D (Hybrid). Rust natively parses `toml` for Python and Rust for maximum speed. For Elixir, Rust safely delegates to an external `mix` script to evaluate the dynamic `mix.exs` AST safely without hallucinations. The results are injected as `Project` nodes and `DEPENDS_ON` edges into KuzuDB.

**Tech Stack:** Rust (toml crate, std::process), Elixir (Mix.Project API), KuzuDB Cypher.

---

### Task 1: Create the Elixir Dependency Extractor Script

**Files:**
- Create: `scripts/extract_elixir_deps.exs`
- Create: `tests/test_elixir_extractor.py`

**Step 1: Write the failing test**

```python
import subprocess
import json
import os

def test_elixir_extractor():
    # Setup dummy mix project
    os.makedirs("/tmp/dummy_umbrella/apps/child_a", exist_key=True)
    with open("/tmp/dummy_umbrella/apps/child_a/mix.exs", "w") as f:
        f.write("""
        defmodule ChildA.MixProject do
          use Mix.Project
          def project do
            [app: :child_a, deps: [{:child_b, in_umbrella: true}, {:external, path: "../external"}]]
          end
        end
        """)
    
    result = subprocess.run(
        ["elixir", "scripts/extract_elixir_deps.exs", "/tmp/dummy_umbrella/apps/child_a"], 
        capture_output=True, text=True
    )
    
    assert result.returncode == 0, f"Script failed: {result.stderr}"
    data = json.loads(result.stdout)
    assert data["node"] == "child_a"
    assert len(data["edges"]) == 2
    assert any(e["to"] == "child_b" for e in data["edges"])
```

**Step 2: Run test to verify it fails**

Run: `python3 tests/test_elixir_extractor.py`
Expected: FAIL (File not found or syntax error)

**Step 3: Write minimal implementation**

```elixir
# scripts/extract_elixir_deps.exs
project_dir = Enum.at(System.argv(), 0, ".")
mix_file = Path.join(project_dir, "mix.exs")

unless File.exists?(mix_file) do
  IO.puts(Jason.encode!(%{node: nil, edges: []}))
  System.halt(0)
end

Code.require_file(mix_file)
Mix.Project.in_project(:graph_extractor, project_dir, fn _ ->
  config = Mix.Project.config()
  app_name = config[:app]
  umbrella_apps = Mix.Project.apps_paths() || %{}
  deps = config[:deps] || []
  
  edges = Enum.reduce(deps, [], fn dep, acc ->
    case dep do
      {target, opts} when is_list(opts) -> 
        path = if opts[:in_umbrella], do: "../#{target}", else: opts[:path]
        if path, do: [%{to: to_string(target), path: Path.expand(path, project_dir)}] ++ acc, else: acc
      {target, _vsn, opts} when is_list(opts) -> 
        path = if opts[:in_umbrella], do: "../#{target}", else: opts[:path]
        if path, do: [%{to: to_string(target), path: Path.expand(path, project_dir)}] ++ acc, else: acc
      _ -> acc
    end
  end)

  IO.puts(Jason.encode!(%{node: to_string(app_name), edges: edges}))
end)
```

**Step 4: Run test to verify it passes**

Run: `python3 tests/test_elixir_extractor.py`
Expected: PASS

**Step 5: Commit**

```bash
git add scripts/extract_elixir_deps.exs tests/test_elixir_extractor.py
git commit -m "feat(federation): add elixir dependency extractor script"
```

---

### Task 2: Implement Rust TOML Parser for Python/Rust & Graph Integration

**Files:**
- Modify: `src/axon-core/Cargo.toml`
- Modify: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/graph.rs`

**Step 1: Write the failing test in Rust**

```rust
// In src/axon-core/src/scanner.rs
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_python_toml_extraction() {
        let toml = r#"
        [tool.poetry.dependencies]
        my_local_lib = { path = "../my_local_lib" }
        "#;
        let deps = extract_toml_dependencies(toml);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, "my_local_lib");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_python_toml_extraction`
Expected: FAIL (function not found)

**Step 3: Write minimal implementation**

Add `toml = "0.8"` to Cargo.toml.
Implement `extract_toml_dependencies` using regex or basic `toml::Value` parsing to find `path` keys under dependencies.
Integrate execution of `elixir scripts/extract_elixir_deps.exs` inside the scanner loop when it finds a `mix.exs`.
In `graph.rs`, add `execute_param("MERGE (p1:Project {name: $from}) MERGE (p2:Project {name: $to}) MERGE (p1)-[:DEPENDS_ON]->(p2)", ...)`

**Step 4: Run test to verify it passes**

Run: `cargo test test_python_toml_extraction`
Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/
git commit -m "feat(federation): implement hybrid dependency extraction and graph insertion"
```