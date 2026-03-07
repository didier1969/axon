import sys
import logging
import struct
import msgpack
from typing import Any, Dict

from axon.config.languages import get_language
from axon.core.ingestion.parser_phase import get_parser

logging.basicConfig(level=logging.INFO, stream=sys.stderr)
logger = logging.getLogger("axon.worker")

class AxonWorker:
    """
    Stateless Parser Worker (Pod B) via MsgPack.
    """
    def handle_command(self, command: Dict[str, Any]) -> Dict[str, Any]:
        # MsgPack string keys might be decoded as bytes depending on settings,
        # but we use raw=False in msgpack.unpackb to get strings.
        cmd_type = command.get("command")
        
        try:
            if cmd_type == "ping":
                return {"status": "ok", "data": "pong"}
            elif cmd_type == "parse":
                return self._parse_file(command)
            elif cmd_type == "parse_batch":
                return self._parse_batch(command)
            else:
                return {"status": "error", "message": f"Unknown command: {cmd_type}"}
        except Exception as e:
            logger.exception("Error during command execution")
            return {"status": "error", "message": str(e)}

    def _parse_file(self, command: Dict[str, Any]) -> Dict[str, Any]:
        path = command.get("path")
        content = command.get("content")
        
        if not path or content is None:
            return {"status": "error", "message": "Missing 'path' or 'content' in parse command"}
            
        language = get_language(path)
        parser = get_parser(language)
        
        if not parser:
            return {"status": "error", "message": f"No parser found for language: {language}"}
            
        parse_result = parser.parse(content, path)
        
        symbols = []
        for sym in parse_result.symbols:
            symbols.append({
                "name": sym.name,
                "kind": sym.kind,
                "start_line": sym.start_line,
                "end_line": sym.end_line,
                "content": sym.content
            })
            
        return {
            "status": "ok",
            "data": {
                "path": path,
                "language": language,
                "symbols": symbols
            }
        }

    def _parse_batch(self, command: Dict[str, Any]) -> Dict[str, Any]:
        files = command.get("files", [])
        results = []
        for f in files:
            res = self._parse_file({"command": "parse", **f})
            if res["status"] == "ok":
                results.append(res["data"])
            else:
                logger.error(f"Failed to parse {f.get('path')}: {res.get('message')}")
                
        return {"status": "ok", "data": results}

def main():
    worker = AxonWorker()
    
    # Lecture binaire (Erlang {packet, 4} protocol)
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer

    while True:
        # Lire les 4 octets de taille (Big Endian)
        length_bytes = stdin.read(4)
        if not length_bytes:
            break # Fin du pipe
            
        if len(length_bytes) < 4:
            logger.error("Incomplete length prefix received")
            break

        length = struct.unpack('>I', length_bytes)[0]
        
        # Lire le payload exact
        payload = stdin.read(length)
        if len(payload) < length:
            logger.error("Incomplete payload received")
            break
            
        try:
            command = msgpack.unpackb(payload, raw=False)
            response = worker.handle_command(command)
            
            out_payload = msgpack.packb(response, use_bin_type=True)
            out_length = struct.pack('>I', len(out_payload))
            
            stdout.write(out_length)
            stdout.write(out_payload)
            stdout.flush()
        except msgpack.UnpackException:
            logger.error("Failed to decode MsgPack from stdin")
        except Exception:
            logger.exception("Unexpected error in main loop")

if __name__ == "__main__":
    main()
