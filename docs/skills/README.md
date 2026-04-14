# Axon Skills Registry (Project-local)

This directory contains project-local operator skills.

Canonical project-local skill:
- `docs/skills/axon-engineering-protocol/SKILL.md`

## Runtime linking model

The effective global skill source of truth for Claude/Codex operators is:
- `~/.claude/skills`

Codex discovery path:
- `~/.codex/skills`

Bridge policy (already in place on this machine):
- one symlink per skill from `~/.claude/skills/<skill>` to `~/.codex/skills/<skill>`

Reference:
- `/home/dstadel/.codex/memories/claude-skills-bridge.md`

## Why keep a project-local copy

- versioned with the Axon repository
- reviewable in PRs
- can be mirrored to global skills registry after review
