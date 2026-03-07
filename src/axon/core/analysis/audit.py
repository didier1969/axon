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
    exposure_path: List[str] = None  # List of symbols from entry point to target
    remediation: str = ""  # Suggested code or command to fix the issue
    count: int = 1  # For clustering similar reports

class AuditEngine:
    """Standardized architectural audit engine for Axon."""

    def __init__(self, graph: KnowledgeGraph):
        self.graph = graph

    def _trace_exposure(self, target_id: str) -> List[str]:
        """Find the shortest path from any entry point to target_id."""
        entry_points = [n.id for n in self.graph.get_nodes_by_label(NodeLabel.FUNCTION) if n.is_entry_point]
        if not entry_points:
            entry_points = [n.id for n in self.graph.get_nodes_by_label(NodeLabel.METHOD) if n.is_entry_point]
            
        if not entry_points:
            return []

        # Simple BFS to find exposure path
        queue = [[ep] for ep in entry_points]
        visited = set(entry_points)
        
        while queue:
            path = queue.pop(0)
            current = path[-1]
            
            if current == target_id:
                return path
            
            for rel in self.graph.get_outgoing(current, RelType.CALLS):
                if rel.target not in visited:
                    visited.add(rel.target)
                    queue.append(path + [rel.target])
        
        return []

    def run_all(self, cluster: bool = True) -> List[AuditReport]:
        reports = []
        reports.extend(self._check_semantic_gaps())
        reports.extend(self._check_structural_twins())
        reports.extend(self._check_fragile_boundaries())
        reports.extend(self._check_owasp_rules())
        
        if cluster:
            return self._cluster_reports(reports)
        return reports

    def _cluster_reports(self, reports: List[AuditReport]) -> List[AuditReport]:
        """Group similar reports to reduce noise, especially for large scale projects."""
        if not reports:
            return []

        clustered: Dict[str, AuditReport] = {}
        final_reports: List[AuditReport] = []
        
        # Centrality threshold for ejection: nodes with centrality > 0.15 are never clustered
        CENTRALITY_THRESHOLD = 0.15

        for r in reports:
            # We assume for now check functions return reports with 1 symbol_id
            symbol_id = r.symbol_ids[0]
            node = self.graph.get_node(symbol_id)
            
            # Ejection Rule: If node is a high-centrality hub, don't cluster it
            if node and node.centrality > CENTRALITY_THRESHOLD:
                final_reports.append(r)
                continue

            folder = "unknown"
            if node and node.file_path:
                parts = node.file_path.split('/')
                folder = "/".join(parts[:-1]) if len(parts) > 1 else "."

            # Specific clustering patterns
            key: Optional[str] = None
            msg = r.message

            if r.type == "STRUCTURAL_TWIN" and (".paul" in folder or "handoffs" in folder):
                key = f"{r.type}:{folder}:{r.severity}"
                msg = f"Multiple structural twins detected in {folder} (Template/Document pattern)."
            elif r.type == "OWASP_A07_AUTH_GAP" and node and ("tests/" in (node.file_path or "")):
                key = f"{r.type}:tests:{r.severity}"
                msg = f"Auth Gap: Multiple test files do not interact with security modules (Expected)."
            elif r.type == "STRUCTURAL_TWIN" and folder == ".":
                # Don't cluster root files twins too aggressively
                key = None
            
            if key:
                if key in clustered:
                    clustered[key].count += 1
                    clustered[key].symbol_ids.extend(r.symbol_ids)
                else:
                    r.message = msg
                    clustered[key] = r
            else:
                final_reports.append(r)
                
        return final_reports + list(clustered.values())

    def _check_owasp_rules(self) -> List[AuditReport]:
        """Core OWASP security checks based on architectural patterns."""
        reports = []
        reports.extend(self._check_a01_access_control())
        reports.extend(self._check_a03_injection_risk())
        reports.extend(self._check_a07_auth_gaps())
        return reports

    def _check_a01_access_control(self) -> List[AuditReport]:
        """OWASP A01: Detect sensitive operations missing authorization guards."""
        reports = []
        sensitive_keywords = {"delete", "remove", "update", "admin", "config", "secret", "grant"}
        auth_keywords = {"auth", "permission", "authorize", "guard", "login", "session", "check_user"}
        
        for node in self.graph.get_nodes_by_label(NodeLabel.FUNCTION):
            name_lower = node.name.lower()
            if any(k in name_lower for k in sensitive_keywords):
                has_auth = any(ak in node.content.lower() for ak in auth_keywords)
                if not has_auth:
                    for rel in self.graph.get_incoming(node.id, RelType.CALLS):
                        caller = self.graph.get_node(rel.source)
                        if caller and any(ak in caller.content.lower() for ak in auth_keywords):
                            has_auth = True
                            break
                
                if not has_auth:
                    reports.append(AuditReport(
                        type="OWASP_A01_ACCESS_CONTROL",
                        symbol_ids=[node.id],
                        message=f"Security Risk: Sensitive function '{node.name}' has no visible authorization guard.",
                        severity="High",
                        exposure_path=self._trace_exposure(node.id),
                        remediation=f"Wrap '{node.name}' with an authorization check or decorator (ex: @require_auth)."
                    ))
        return reports

    def _check_a03_injection_risk(self) -> List[AuditReport]:
        """OWASP A03: Detect dangerous sinks accessible from public entry points."""
        reports = []
        sinks = {"execute", "query", "eval", "system", "os.spawn", "command", "dangerous_"}
        sanitizers = {"sanitize", "escape", "quote", "filter", "validate", "encode"}
        
        for node in self.graph.get_nodes_by_label(NodeLabel.FUNCTION):
            name_lower = node.name.lower()
            if any(s in name_lower for s in sinks):
                path = self._trace_exposure(node.id)
                if path:
                    if not any(sz in node.content.lower() for sz in sanitizers):
                        reports.append(AuditReport(
                            type="OWASP_A03_INJECTION",
                            symbol_ids=[node.id],
                            message=f"Injection Risk: Dangerous sink '{node.name}' is exposed to public entry points without visible sanitization.",
                            severity="Critical",
                            exposure_path=path,
                            remediation="Use parameterized queries or pass input through a sanitization function."
                        ))
        return reports

    def _check_a07_auth_gaps(self) -> List[AuditReport]:
        """OWASP A07: Entry points completely disconnected from Auth modules."""
        reports = []
        entry_points = [n for n in self.graph.get_nodes_by_label(NodeLabel.FUNCTION) if n.is_entry_point]
        
        for ep in entry_points:
            has_security_dep = False
            for rel in self.graph.get_outgoing(ep.id):
                target = self.graph.get_node(rel.target)
                if target and ("auth" in target.name.lower() or "session" in target.name.lower()):
                    has_security_dep = True
                    break
            
            if not has_security_dep:
                reports.append(AuditReport(
                    type="OWASP_A07_AUTH_GAP",
                    symbol_ids=[ep.id],
                    message=f"Auth Gap: Entry point '{ep.name}' does not seem to interact with any security/auth modules.",
                    severity="Medium",
                    remediation="Verify if this endpoint requires authentication and link it to the Auth system."
                ))
        return reports

    def _check_semantic_gaps(self) -> List[AuditReport]:
        """Detect functions with 'heavy' names but 'light' implementation."""
        reports = []
        keywords = {"persist", "save", "flush", "initialize", "sync", "commit"}
        
        for node in self.graph.get_nodes_by_label(NodeLabel.FUNCTION):
            name_lower = node.name.lower()
            if any(k in name_lower for k in keywords):
                if len(node.content.strip()) < 40 and node.centrality > 0.1:
                    reports.append(AuditReport(
                        type="SEMANTIC_GAP",
                        symbol_ids=[node.id],
                        message=f"Function '{node.name}' seems to be a shallow implementation (Stub) despite its high importance.",
                        severity="High"
                    ))
        return reports

    def _check_structural_twins(self) -> List[AuditReport]:
        """Detect duplicate logic in different files (Divergence Risk)."""
        reports = []
        content_map = {}
        for node in self.graph.iter_nodes():
            if node.label in {NodeLabel.FUNCTION, NodeLabel.METHOD}:
                if len(node.content) > 5:
                    if node.content in content_map:
                        twin = content_map[node.content]
                        if twin.file_path != node.file_path:
                            reports.append(AuditReport(
                                type="STRUCTURAL_TWIN",
                                symbol_ids=[node.id],
                                message=f"Divergence risk: '{node.name}' is a structural twin of '{twin.name}' in {twin.file_path}.",
                                severity="Medium",
                                remediation=f"Consolidate logic. Consider removing {node.file_path} or merging it with {twin.file_path}."
                            ))
                    content_map[node.content] = node
        return reports

    def _check_fragile_boundaries(self) -> List[AuditReport]:
        """Detect calls to external/native code without validation guards."""
        reports = []
        guards = {"is_list", "is_map", "is_binary", "is_integer", "is_struct", "@type", "assert"}
        
        for node in self.graph.get_nodes_by_label(NodeLabel.FUNCTION):
            for rel in self.graph.get_outgoing(node.id):
                target = self.graph.get_node(rel.target)
                if target and target.file_path:
                    path_lower = target.file_path.lower()
                    if "bridge" in path_lower or "nif" in path_lower:
                        if not any(g in node.content for g in guards):
                            exposure = self._trace_exposure(node.id)
                            reports.append(AuditReport(
                                type="FRAGILE_BOUNDARY",
                                symbol_ids=[node.id],
                                message=f"Fragile boundary: '{node.name}' calls native code without data validation guards.",
                                severity="High",
                                exposure_path=exposure,
                                remediation=f"Add data validation guards before calling {target.name}."
                            ))
        return reports
