import logging
import socket
import struct
import msgpack
from pathlib import Path
from typing import Any, Iterable, List, Optional

from axon.core.graph.graph import KnowledgeGraph
from axon.core.graph.model import GraphNode, GraphRelationship, NodeLabel
from axon.core.storage.base import StorageBackend, SearchResult, NodeEmbedding

logger = logging.getLogger("axon.core.storage.astral")

class AstralBackend(StorageBackend):
    """
    HydraDB (Astral DB) High-Performance TCP Backend for Axon v1.0.
    Communicates with Pod C (HydraDB) via TCP Socket + MsgPack on port 4040.
    """

    def __init__(self, host: str = "127.0.0.1", port: int = 4040, timeout: int = 30):
        self.host = host
        self.port = port
        self.timeout = timeout
        self._socket: Optional[socket.socket] = None

    def _connect(self):
        """Establish or verify TCP connection."""
        if self._socket is None:
            try:
                self._socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                self._socket.settimeout(self.timeout)
                self._socket.connect((self.host, self.port))
                logger.info(f"Connected to HydraDB TCP Interface at {self.host}:{self.port}")
            except ConnectionRefusedError:
                logger.error(f"HydraDB (Pod C) refused TCP connection at {self.host}:{self.port}")
                raise RuntimeError("HydraDB TCP Interface is not reachable.")

    def _send_command(self, cmd: str, args: dict) -> Any:
        """Serialize and send command over TCP with length prefix."""
        self._connect()
        # v1.0 Protocol: 4 bytes length (big-endian) + MsgPack payload
        payload = msgpack.packb({"command": cmd, "args": args}, use_bin_type=True)
        length_prefix = struct.pack('>I', len(payload))
        
        try:
            self._socket.sendall(length_prefix + payload)
            
            # Read response length
            resp_len_bytes = self._socket.recv(4)
            if not resp_len_bytes: return None
            resp_len = struct.unpack('>I', resp_len_bytes)[0]
            
            # Read response payload
            resp_payload = b""
            while len(resp_payload) < resp_len:
                chunk = self._socket.recv(min(resp_len - len(resp_payload), 16384))
                if not chunk: break
                resp_payload += chunk
                
            response = msgpack.unpackb(resp_payload, raw=False)
            if response.get("status") == "error":
                raise RuntimeError(f"HydraDB Error: {response.get('message')}")
            return response.get("data")
        except (socket.error, struct.error) as e:
            logger.warning(f"TCP connection lost, retrying... ({e})")
            self._socket.close()
            self._socket = None
            return self._send_command(cmd, args) # Recursive retry once

    def initialize(self, path: Path | str, read_only: bool = False) -> None:
        """Handshake with HydraDB."""
        # Treatment of 'path' as repo context in v1.0
        repo_name = Path(path).name
        self._send_command("initialize_repo", {"name": repo_name, "read_only": read_only})

    def execute_raw(self, query: str, parameters: dict = None) -> Any:
        """Execute ultra-fast graph query."""
        return self._send_command("query", {"query": query, "params": parameters or {}})

    def get_node(self, node_id: str) -> GraphNode | None:
        data = self._send_command("get_node", {"id": node_id})
        return GraphNode(**data) if data else None

    def traverse(self, start_id: str, depth: int, direction: str = "outgoing") -> list[GraphNode]:
        """Server-side deep traversal."""
        nodes_data = self._send_command("traverse", {
            "start_node": start_id,
            "max_depth": depth,
            "direction": direction
        })
        return [GraphNode(**n) for n in (nodes_data or [])]

    def bulk_load(self, nodes: Iterable[GraphNode], rels: Iterable[GraphRelationship]) -> None:
        """Stream batch data over TCP."""
        node_list = [n.to_dict() if hasattr(n, "to_dict") else n.__dict__ for n in nodes]
        rel_list = [r.to_dict() if hasattr(r, "to_dict") else r.__dict__ for r in rels]
        
        # Binary bulk load is significantly faster
        self._send_command("bulk_load", {"nodes": node_list, "relationships": rel_list})

    def close(self) -> None:
        if self._socket:
            self._socket.close()
            self._socket = None

    def remove_nodes_by_file(self, file_path: str) -> int:
        return self._send_command("remove_file", {"path": file_path}) or 0

    def vector_search(self, vector: list[float], limit: int) -> list[SearchResult]:
        results = self._send_command("vector_search", {"vector": vector, "limit": limit})
        return [SearchResult(**r) for r in (results or [])]

    def get_indexed_files(self) -> dict[str, str]:
        return self._send_command("get_files", {}) or {}
