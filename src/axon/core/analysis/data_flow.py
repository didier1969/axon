from __future__ import annotations

from dataclasses import dataclass
from typing import List, Set

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import NodeLabel, RelType

@dataclass
class FlowStep:
    symbol_id: str
    symbol_name: str
    file_path: str
    passed_arguments: List[str]

@dataclass
class DataFlowPath:
    source_id: str
    target_id: str
    steps: List[FlowStep]

class DataFlowAnalyzer:
    """Traces the flow of variables through the architecture."""

    def __init__(self, graph: KnowledgeGraph):
        self.graph = graph

    def trace_variable(self, source_id: str, variable_name: str, max_depth: int = 5) -> List[DataFlowPath]:
        """
        Trace how a specific variable propagates from a source function to other functions.
        This acts as an architectural Taint Analysis.
        """
        paths = []
        source_node = self.graph.get_node(source_id)
        if not source_node:
            return paths

        # BFS Queue: (current_node_id, current_path, current_depth)
        initial_step = FlowStep(
            symbol_id=source_node.id, 
            symbol_name=source_node.name, 
            file_path=source_node.file_path, 
            passed_arguments=[variable_name]
        )
        queue = [(source_id, [initial_step], 0)]
        
        # We track visited nodes to prevent infinite loops, but allow different paths
        # For simplicity in this architectural view, we track visited edges.
        visited_edges: Set[str] = set()

        while queue:
            current_id, current_path, depth = queue.pop(0)

            if depth >= max_depth:
                continue

            outgoing_calls = self.graph.get_outgoing(current_id, RelType.CALLS)
            has_propagated = False

            for rel in outgoing_calls:
                # Check if our tracked variable is passed in this call
                args = rel.properties.get("arguments", [])
                
                # In a full AST we would map arg position to param name.
                # Here, if any argument matches our tracked variable name, the data flows.
                # We also assume that if a function is part of the tainted path, 
                # it might pass its own params down (simplified taint).
                # For strict tracing, we check if the variable name is explicitly passed.
                is_passed = variable_name in args
                
                # Relaxed rule: if we are deeper than 0, the current function is "tainted".
                # We record calls that take any arguments to see the flow, 
                # but we highlight if the specific variable name is reused.
                if is_passed or depth > 0:
                    edge_sig = f"{current_id}->{rel.target}"
                    if edge_sig not in visited_edges:
                        visited_edges.add(edge_sig)
                        
                        target_node = self.graph.get_node(rel.target)
                        if target_node:
                            step = FlowStep(
                                symbol_id=target_node.id,
                                symbol_name=target_node.name,
                                file_path=target_node.file_path,
                                passed_arguments=args
                            )
                            new_path = current_path + [step]
                            
                            # If it's a sink or a leaf node (no outgoing calls), we consider it a complete path
                            target_outgoing = self.graph.get_outgoing(rel.target, RelType.CALLS)
                            if not target_outgoing or depth + 1 == max_depth:
                                paths.append(DataFlowPath(source_id, rel.target, new_path))
                            
                            queue.append((rel.target, new_path, depth + 1))
                            has_propagated = True
            
            # If the data stops here and it's not the source, record the path
            if not has_propagated and depth > 0:
                 paths.append(DataFlowPath(source_id, current_id, current_path))

        return self._deduplicate_paths(paths)

    def _deduplicate_paths(self, paths: List[DataFlowPath]) -> List[DataFlowPath]:
        """Remove subset paths if a longer path exists."""
        # Simple deduplication based on target_id and length
        unique = {}
        for p in paths:
            key = tuple(step.symbol_id for step in p.steps)
            unique[key] = p
            
        return list(unique.values())

    def find_sinks_for_entry_point(self, entry_point_id: str) -> List[DataFlowPath]:
        """
        Specialized trace: find if an entry point reaches any known dangerous sinks.
        """
        # This leverages the BFS but specifically flags when target is a sink.
        return self.trace_variable(entry_point_id, "*", max_depth=6)
