#!/usr/bin/env python3
"""Shared runtime role and authority contracts for Axon operator scripts."""

from __future__ import annotations

from typing import Any


SUPPORTED_MODES = {
    "brain_only",
    "indexer_graph",
    "indexer_vector",
    "indexer_full",
    "brain",
    "indexer",
}


def runtime_authority_contract(process_role: str | None) -> dict[str, str]:
    role = "brain" if process_role in {None, "", "brain", "brain_only"} else "indexer"
    return {
        "process_role": role,
        "public_mcp_authority": "brain",
        "soll_writer_authority": "brain",
        "ist_writer_authority": "indexer",
    }


def mode_contract(mode: str) -> dict[str, Any]:
    normalized = mode.replace("-", "_")
    if normalized in {"brain_only", "brain"}:
        return {
            "shadow_role": "brain",
            "shadow_only": False,
            "runtime_mode": "brain_only",
            "start_script": "scripts/lib/start-brain.sh",
            "authority_contract": runtime_authority_contract("brain"),
        }
    if normalized in {"indexer_graph", "indexer_vector", "indexer_full", "indexer"}:
        runtime_mode = {
            "indexer": "indexer_graph",
        }.get(normalized, normalized)
        return {
            "shadow_role": "indexer",
            "shadow_only": False,
            "runtime_mode": runtime_mode,
            "start_script": "scripts/lib/start-indexer.sh",
            "authority_contract": runtime_authority_contract("indexer"),
        }
    return {
        "shadow_role": "indexer",
        "shadow_only": False,
        "runtime_mode": "indexer_graph",
        "start_script": "scripts/lib/start-indexer.sh",
        "authority_contract": runtime_authority_contract("indexer"),
    }
