import os
import sys
import logging
import socket
import struct
import msgpack
from pathlib import Path
from typing import Any, Dict

from axon.config.languages import get_language
from axon.core.ingestion.parser_phase import get_parser

# Configuration simplifiée pour le worker
logging.basicConfig(level=logging.INFO, format="%(asctime)s [Worker] %(message)s")
logger = logging.getLogger("axon.worker")

class AxonWorkerUDS:
    """
    High-performance Parser Worker (Pod B) using Unix Domain Sockets.
    """
    def __init__(self, socket_path: str = "/tmp/axon-parser.sock"):
        self.socket_path = socket_path

    def _parse_file(self, command: Dict[str, Any]) -> Dict[str, Any]:
        path = command.get("path")
        language = command.get("language")
        content = command.get("content")

        if not path or not content:
            return {"status": "error", "message": "Missing path or content"}

        if not language:
            lang_config = get_language(Path(path).suffix)
            language = lang_config.name if lang_config else "text"

        parser = get_parser(language)
        if not parser:
            return {"status": "error", "message": f"Unsupported language: {language}"}

        try:
            parse_result = parser.parse(content, path)
        except Exception as e:
            return {"status": "error", "message": str(e)}

        symbols = []
        for sym in parse_result.symbols:
            symbols.append({
                "id": getattr(sym, "id", f"fallback:{sym.name}"),
                "name": sym.name,
                "kind": sym.kind,
                "start_line": sym.start_line,
                "end_line": sym.end_line,
                "start_byte": getattr(sym, "start_byte", 0),
                "end_byte": getattr(sym, "end_byte", 0),
                "content": sym.content,
                "is_exported": getattr(sym, "is_exported", False),
                "is_entry_point": getattr(sym, "is_entry_point", False),
                "signature": getattr(sym, "signature", ""),
                "tested": getattr(sym, "tested", False),
                "centrality": getattr(sym, "centrality", 0.0)
            })
            
        relationships = []
        for rel in getattr(parse_result, "relationships", []):
            relationships.append({
                "source": rel.source,
                "target": rel.target,
                "type": rel.type.value if hasattr(rel.type, "value") else str(rel.type),
                "properties": rel.properties
            })
            
        return {
            "status": "ok",
            "data": {
                "path": path,
                "language": language,
                "symbols": symbols,
                "relationships": relationships
            }
        }

    def _handle_client(self, conn):
        with conn:
            while True:
                try:
                    # Read 4-byte length prefix
                    header = conn.recv(4)
                    if not header: break
                    length = struct.unpack(">I", header)[0]
                    
                    # Read MsgPack payload
                    data = b""
                    while len(data) < length:
                        chunk = conn.recv(min(length - len(data), 16384))
                        if not chunk: break
                        data += chunk
                    
                    if not data: break
                    
                    request = msgpack.unpackb(data, raw=False)
                    cmd_type = request.get("command")
                    
                    if cmd_type == "parse":
                        response = self._parse_file(request.get("args", {}))
                    elif cmd_type == "ping":
                        response = {"status": "ok", "data": "pong"}
                    else:
                        response = {"status": "error", "message": f"Unknown command: {cmd_type}"}
                    
                    # Send response with length prefix
                    resp_payload = msgpack.packb(response, use_bin_type=True)
                    conn.sendall(struct.pack(">I", len(resp_payload)) + resp_payload)
                except Exception as e:
                    logger.error(f"Error handling request: {e}")
                    break

    def run(self):
        # Cleanup old socket
        if os.path.exists(self.socket_path):
            os.remove(self.socket_path)
            
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(self.socket_path)
        os.chmod(self.socket_path, 0o666) # Ensure Elixir can write
        server.listen(10)
        
        logger.info(f"Pod B Parser listening on UDS: {self.socket_path}")
        
        try:
            while True:
                conn, _ = server.accept()
                self._handle_client(conn)
        except KeyboardInterrupt:
            pass
        finally:
            server.close()
            if os.path.exists(self.socket_path):
                os.remove(self.socket_path)

if __name__ == "__main__":
    worker = AxonWorkerUDS()
    worker.run()
