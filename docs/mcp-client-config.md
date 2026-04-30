# Axon MCP Client Configuration

## Claude Code (CLI / Desktop / Web)

Add to `~/.claude/settings.json` or project `.claude/settings.json`:

```json
{
  "mcpServers": {
    "axon": {
      "command": "/path/to/axon-brain",
      "args": ["--mcp"],
      "env": {
        "AXON_INSTANCE_KIND": "live",
        "AXON_STATE_ROOT": "/path/to/project/.axon"
      }
    }
  }
}
```

## VS Code (Continue / Cline)

Add to `.vscode/settings.json`:

```json
{
  "continue.mcpServers": {
    "axon": {
      "command": "/path/to/axon-brain",
      "args": ["--mcp"]
    }
  }
}
```

## Cursor

Add to `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "axon": {
      "command": "/path/to/axon-brain",
      "args": ["--mcp"],
      "env": {
        "AXON_STATE_ROOT": "/path/to/project/.axon"
      }
    }
  }
}
```

## Windsurf

Add to `~/.windsurf/mcp.json` (same format as Cursor).

## First Use

After connecting, the LLM should:
1. Call `help()` — returns Axon identity, tool routing, input schemas
2. Call `status()` — returns runtime truth and auto-detected `project_code`
3. Start working with `query("symbol_name")`

No `CLAUDE.md`, skill files, or memory configuration required. `project_code` is auto-detected from the working directory.
