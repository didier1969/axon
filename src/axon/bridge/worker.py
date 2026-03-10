import os
import sys
import struct
import msgpack
from pathlib import Path
from typing import Any, Dict, List, Optional
from pydantic import BaseModel, Field
from loguru import logger

from axon.config.languages import get_language
from axon.core.ingestion.parser_phase import get_parser

# Configuration Loguru pour Pod B
logger.remove()
logger.add(sys.stderr, format="<green>{time:YYYY-MM-DD HH:mm:ss}</green> | <level>{level: <8}</level> | [Pod B] <level>{message}</level>")

class SymbolModel(BaseModel):
    id: str
    name: str
    kind: str
    start_line: int
    end_line: int
    start_byte: int = 0
    end_byte: int = 0
    content: str = ""
    is_exported: bool = False
    is_entry_point: bool = False
    signature: str = ""
    tested: bool = False
    centrality: float = 0.0

class RelationshipModel(BaseModel):
    source: str
    target: str
    type: str
    properties: Dict[str, Any] = Field(default_factory=dict)

class FileExtractionResult(BaseModel):
    path: str
    language: str
    symbols: List[SymbolModel] = Field(default_factory=list)
    relationships: List[RelationshipModel] = Field(default_factory=list)

class AxonWorkerPort:
    """
    High-performance Parser Worker (Pod B) using Erlang Ports (stdin/stdout).
    Rigidified with Pydantic validation.
    """
    def _parse_file(self, command: Dict[str, Any]) -> Dict[str, Any]:
        path = command.get("path")
        language = command.get("language")
        content = command.get("content")

        if not path or not content:
            logger.error(f"Missing path or content for: {path}")
            return {"status": "error", "message": "Missing path or content"}

        if not language:
            language = get_language(Path(path))

        parser = get_parser(language)
        if not parser:
            logger.warning(f"Unsupported language: {language} for {path}")
            return {"status": "error", "message": f"Unsupported language: {language}"}

        try:
            if isinstance(content, bytes):
                content = content.decode('utf-8', errors='replace')
            parse_result = parser.parse(content, path)
        except Exception as e:
            logger.exception(f"Parser crash for {path}: {e}")
            return {"status": "error", "message": str(e)}

        try:
            symbols = []
            for sym in parse_result.symbols:
                symbols.append(SymbolModel(
                    id=getattr(sym, "id", f"fallback:{sym.name}"),
                    name=sym.name,
                    kind=sym.kind,
                    start_line=sym.start_line,
                    end_line=sym.end_line,
                    start_byte=getattr(sym, "start_byte", 0),
                    end_byte=getattr(sym, "end_byte", 0),
                    content=sym.content or "",
                    is_exported=getattr(sym, "is_exported", False),
                    is_entry_point=getattr(sym, "is_entry_point", False),
                    signature=getattr(sym, "signature", ""),
                    tested=getattr(sym, "tested", False),
                    centrality=float(getattr(sym, "centrality", 0.0))
                ))
                
            relationships = []
            for rel in getattr(parse_result, "relationships", []):
                rel_type = rel.type.value if hasattr(rel.type, "value") else str(rel.type)
                relationships.append(RelationshipModel(
                    source=rel.source,
                    target=rel.target,
                    type=rel_type,
                    properties=rel.properties or {}
                ))
            
            # Validation finale via Pydantic
            result = FileExtractionResult(
                path=path,
                language=language,
                symbols=symbols,
                relationships=relationships
            )
            return {"status": "ok", "data": result.model_dump()}

        except Exception as e:
            logger.error(f"Validation error for {path}: {e}")
            return {"status": "error", "message": f"Validation failed: {e}"}

    def _parse_batch(self, command: Dict[str, Any]) -> Dict[str, Any]:
        files = command.get("files", [])
        results = []
        for f in files:
            res = self._parse_file(f)
            if res.get("status") == "ok":
                results.append(res["data"])
        return {"status": "ok", "data": results}

    def run(self):
        logger.info("Pod B Parser starting in Erlang Port mode (stdin/stdout)")
        while True:
            try:
                # Read 4-byte length prefix
                header = sys.stdin.buffer.read(4)
                if not header or len(header) < 4:
                    break
                
                length = struct.unpack(">I", header)[0]
                payload = sys.stdin.buffer.read(length)
                
                command = msgpack.unpackb(payload, raw=False)
                op = command.get("op", "parse_file")
                
                if op == "parse_file":
                    response = self._parse_file(command)
                elif op == "parse_batch":
                    response = self._parse_batch(command)
                else:
                    response = {"status": "error", "message": f"Unknown operation: {op}"}
                
                # Send response back via stdout
                resp_payload = msgpack.packb(response, use_bin_type=True)
                sys.stdout.buffer.write(struct.pack(">I", len(resp_payload)) + resp_payload)
                sys.stdout.buffer.flush()
                
            except EOFError:
                break
            except Exception as e:
                logger.error(f"Bridge loop error: {e}")
                break

if __name__ == "__main__":
    worker = AxonWorkerPort()
    worker.run()
