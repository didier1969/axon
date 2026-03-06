from __future__ import annotations
from dataclasses import dataclass
from typing import List
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import NodeLabel, GraphNode

@dataclass
class AuditReport:
    type: str  # SEMANTIC_GAP, STRUCTURAL_TWIN, FRAGILE_BOUNDARY
    symbol_id: str
    message: str
    severity: str  # High, Medium, Low

class AuditEngine:
    """Standardized architectural audit engine for Axon."""

    def __init__(self, graph: KnowledgeGraph):
        self.graph = graph

    def run_all(self) -> List[AuditReport]:
        reports = []
        reports.extend(self._check_semantic_gaps())
        reports.extend(self._check_structural_twins())
        reports.extend(self._check_fragile_boundaries())
        return reports

    def _check_semantic_gaps(self) -> List[AuditReport]:
        """Detect functions with 'heavy' names but 'light' implementation."""
        reports = []
        keywords = {"persist", "save", "flush", "initialize", "sync", "commit"}
        
        for node in self.graph.get_nodes_by_label(NodeLabel.FUNCTION):
            name_lower = node.name.lower()
            if any(k in name_lower for k in keywords):
                # Threshold: less than 40 chars of content and high centrality
                if len(node.content.strip()) < 40 and node.centrality > 0.1:
                    reports.append(AuditReport(
                        type="SEMANTIC_GAP",
                        symbol_id=node.id,
                        message=f"Function '{node.name}' seems to be a shallow implementation (Stub) despite its high importance.",
                        severity="High"
                    ))
        return reports

    def _check_structural_twins(self) -> List[AuditReport]:
        """Detect duplicate logic in different files (Divergence Risk)."""
        reports = []
        # Simple content-based twin detection for this demo
        content_map = {}
        for node in self.graph.iter_nodes():
            if node.label in {NodeLabel.FUNCTION, NodeLabel.METHOD}:
                if len(node.content) > 5: # Ignore very small helpers
                    if node.content in content_map:
                        twin = content_map[node.content]
                        if twin.file_path != node.file_path:
                            reports.append(AuditReport(
                                type="STRUCTURAL_TWIN",
                                symbol_id=node.id,
                                message=f"Divergence risk: '{node.name}' is a structural twin of '{twin.name}' in {twin.file_path}.",
                                severity="Medium"
                            ))
                    content_map[node.content] = node
        return reports

    def _check_fragile_boundaries(self) -> List[AuditReport]:
        """Detect calls to external/native code without validation guards."""
        reports = []
        guards = {"is_list", "is_map", "is_binary", "is_integer", "is_struct", "@type", "assert"}
        
        for node in self.graph.get_nodes_by_label(NodeLabel.FUNCTION):
            # Find outgoing calls
            for rel in self.graph.get_outgoing(node.id):
                target = self.graph.get_node(rel.target)
                if target and target.file_path:
                    path_lower = target.file_path.lower()
                    if "bridge" in path_lower or "nif" in path_lower:
                        # Potential boundary call. Check for guards in caller content.
                        if not any(g in node.content for g in guards):
                            reports.append(AuditReport(
                                type="FRAGILE_BOUNDARY",
                                symbol_id=node.id,
                                message=f"Fragile boundary: '{node.name}' calls native code without data validation guards.",
                                severity="High"
                            ))
        return reports
