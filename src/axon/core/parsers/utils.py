from __future__ import annotations
from tree_sitter import Node

def find_child_by_type(node: Node, type_name: str) -> Node | None:
    """Find the first child of a node with a specific type name."""
    for child in node.children:
        if child.type == type_name:
            return child
    return None

def get_node_text(node: Node, source_code: str | bytes) -> str:
    """Extract the text of a node from the source code."""
    if isinstance(source_code, str):
        return source_code[node.start_byte:node.end_byte]
    return source_code[node.start_byte:node.end_byte].decode("utf-8", errors="replace")
