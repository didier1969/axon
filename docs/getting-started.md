# Getting Started with Axon

Axon indexes your codebase into a knowledge graph and exposes it as MCP tools so AI agents get structural understanding without reading raw files.

## Prerequisites

- Python 3.11+
- uv or pip
- An AI assistant that supports MCP (Claude Code, Cursor, etc.)

## Step 1: Install

```bash
pip install axoniq
# or with uv (recommended)
uv add axoniq
```

## Step 2: Index your project

```bash
cd your-project
axon analyze .
```

Expected output:

```
Walking files...               142 files found
Parsing code...                142/142
Tracing calls...               847 calls resolved
...
Done in 4.2s — 623 symbols, 1,847 edges, 8 clusters, 34 flows
```

For a faster first run, skip embedding generation:

```bash
axon analyze . --no-embeddings
```

## Step 3: Connect your AI assistant

**Claude Code** — add to `.claude/settings.json` or `.mcp.json`:

```json
{
  "mcpServers": {
    "axon": {
      "command": "axon",
      "args": ["serve", "--watch"]
    }
  }
}
```

Or use the helper: `axon setup --claude`

**Cursor** — add to MCP settings:

```json
{
  "axon": {
    "command": "axon",
    "args": ["serve", "--watch"]
  }
}
```

Or use: `axon setup --cursor`

## Step 4: Verify it's working

In your AI assistant, ask:

- "Use axon_list_repos to show indexed repositories"
- "Use axon_query to find the authentication handler"

The tool should return results from the knowledge graph.

## Step 5 (Optional): Auto-start on project entry

Choose one:

**Shell hook** (bash/zsh — auto-starts watcher when you cd into any indexed project):

```bash
echo 'eval "$(axon shell-hook)"' >> ~/.bashrc
# or for zsh:
echo 'eval "$(axon shell-hook --shell zsh)"' >> ~/.zshrc
```

**direnv** (per-project):

```bash
axon init --direnv
```

## What's next

- [MCP Tool Reference](../README.md#mcp-integration) — all 7 tools with usage patterns
- [CI Integration](../README.md#ci-integration) — dead-code quality gates and template configs
- `axon --help` — full CLI reference
