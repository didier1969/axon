from __future__ import annotations

from axon.core.graph.model import NodeLabel, GraphNode, GraphRelationship

_NODE_TABLE_NAMES: list[str] = [label.name.title().replace("_", "") for label in NodeLabel]
_LABEL_TO_TABLE: dict[str, str] = {
    label.value: label.name.title().replace("_", "") for label in NodeLabel
}
_LABEL_MAP: dict[str, NodeLabel] = {label.value: label for label in NodeLabel}
_SEARCHABLE_TABLES: list[str] = [
    t for t in _NODE_TABLE_NAMES if t not in ("Folder", "Community", "Process")
]

_NODE_PROPERTIES = (
    "id STRING, name STRING, file_path STRING, start_line INT64, "
    "end_line INT64, start_byte INT64, end_byte INT64, content STRING, "
    "content_hash STRING, signature STRING, language STRING, "
    "class_name STRING, is_dead BOOL, is_entry_point BOOL, is_exported BOOL, "
    "tested BOOL DEFAULT false, centrality DOUBLE DEFAULT 0.0, "
    "PRIMARY KEY (id)"
)
_REL_PROPERTIES = (
    "rel_type STRING, confidence DOUBLE, role STRING, step_number INT64, "
    "strength DOUBLE, co_changes INT64, symbols STRING"
)
_EMBEDDING_PROPERTIES = "node_id STRING, vec DOUBLE[], PRIMARY KEY(node_id)"

def get_table_for_id(node_id: str) -> str | None:
    """Extract the table name from a node ID by mapping its label prefix."""
    return _LABEL_TO_TABLE.get(node_id.split(":", 1)[0])

def node_to_row(n: GraphNode) -> list:
    """Convert a GraphNode to a flat row for CSV COPY."""
    return [n.id, n.name, n.file_path, n.start_line, n.end_line,
            n.start_byte, n.end_byte, n.content,
            n.properties.get("content_hash", ""),
            n.signature, n.language, n.class_name, n.is_dead,
            n.is_entry_point, n.is_exported, n.tested, n.centrality]

def rel_to_row(r: GraphRelationship) -> list:
    """Convert a GraphRelationship to a flat row for CSV COPY."""
    props = r.properties or {}
    return [r.source, r.target, r.type.value,
            float(props.get("confidence", 1.0)), str(props.get("role", "")),
            int(props.get("step_number", 0)), float(props.get("strength", 0.0)),
            int(props.get("co_changes", 0)), str(props.get("symbols", ""))]

def escape(value: str) -> str:
    """Escape a string for safe inclusion in a Cypher literal."""
    return value.replace("\\", "\\\\").replace("'", "\\'")
