from __future__ import annotations
import logging
from typing import Union, Any
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, GraphRelationship, NodeLabel

logger = logging.getLogger(__name__)

_KIND_TO_LABEL: dict[str, NodeLabel] = {
    "function": NodeLabel.FUNCTION,
    "class": NodeLabel.CLASS,
    "method": NodeLabel.METHOD,
    "interface": NodeLabel.INTERFACE,
    "type_alias": NodeLabel.TYPE_ALIAS,
    "enum": NodeLabel.ENUM,
    # Elixir
    "module": NodeLabel.CLASS,
    "macro": NodeLabel.FUNCTION,
    "struct": NodeLabel.CLASS,
    # Markdown
    "section": NodeLabel.FUNCTION,
}

def get_node_label(kind: str) -> NodeLabel:
    """Return the NodeLabel for a given symbol kind string."""
    return _KIND_TO_LABEL.get(kind, NodeLabel.FUNCTION)

def add_to_graph(graph: KnowledgeGraph | None, item: Union[GraphNode, GraphRelationship]) -> Union[GraphNode, GraphRelationship]:
    """Add a node or relationship to the graph if it is provided."""
    if graph is not None:
        if isinstance(item, GraphNode):
            graph.add_node(item)
        else:
            graph.add_relationship(item)
    return item
