from __future__ import annotations
from dataclasses import dataclass, field
from typing import List, Dict, Optional
from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import NodeLabel, GraphNode, RelType

@dataclass
class AuditReport:
    type: str  # SEMANTIC_GAP, STRUCTURAL_TWIN, FRAGILE_BOUNDARY
    symbol_ids: List[str]  # List of all symbols impacted by this report
    message: str
    severity: str  # High, Medium, Low
    exposure_path: List[str] = field(default_factory=list)
    remediation: str = ""  # Suggested code or command to fix the issue
    count: int = 1

class AuditEngine:
    """
    Standardized architectural audit engine for Axon v1.0.
    Delegates graph-heavy analysis to the Storage Backend (Pod C).
    """

    def __init__(self, storage: Any):
        self.storage = storage

    def _trace_exposure(self, target_id: str) -> List[str]:
        """Delegates exposure tracing to the backend."""
        try:
            # We call the traverse method of AstralBackend which runs server-side
            path_nodes = self.storage.traverse(target_id, depth=10, direction="callers")
            return [n.id for n in path_nodes]
        except Exception:
            return []

    def run_all(self, cluster: bool = True) -> List[AuditReport]:
        """Runs the audit suite using backend-optimized queries."""
        reports = []
        
        # 1. Access Control Audit (OWASP A01)
        # We use execute_raw to let Pod C handle the heavy filtering
        sensitive_query = """
        MATCH (n:Function) 
        WHERE n.name =~ '.*(delete|remove|admin|secret).*'
        AND NOT n.content =~ '.*(auth|guard|check|session).*'
        RETURN n.id, n.name
        """
        try:
            risks = self.storage.execute_raw(sensitive_query)
            for risk in (risks or []):
                reports.append(AuditReport(
                    type="OWASP_A01_ACCESS_CONTROL",
                    symbol_ids=[risk[0]],
                    message=f"Security Risk: Sensitive function '{risk[1]}' has no visible authorization guard.",
                    severity="High",
                    exposure_path=self._trace_exposure(risk[0]),
                    remediation=f"Wrap '{risk[1]}' with an authorization check."
                ))
        except Exception as e:
            # Silently fail for individual checks to avoid terminal crash
            pass

        return reports
