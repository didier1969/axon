import msgpack
import struct
import subprocess
import pytest
from pathlib import Path

"""
Test E2E pour Axon v1.0 (Modèle Triple-Pod).
Ce test vérifie que l'esclave Python (Pod B) se comporte correctement
lorsqu'il est piloté par un orchestrateur via le protocole binaire MsgPack {packet, 4}.
"""

class TestV1TriplePodE2E:
    
    @pytest.fixture
    def slave_process(self):
        """Lance l'esclave Python en mode worker binaire (Pod B)."""
        cmd = ["uv", "run", "python", "src/axon/bridge/worker.py"]
        process = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
        )
        yield process
        process.terminate()

    def _send_receive(self, process, command: dict) -> dict:
        """Helper pour simuler le protocole Erlang Port {packet, 4} avec MsgPack."""
        payload = msgpack.packb(command, use_bin_type=True)
        length_prefix = struct.pack('>I', len(payload))
        
        process.stdin.write(length_prefix + payload)
        process.stdin.flush()
        
        # Lecture de la réponse
        res_length_bytes = process.stdout.read(4)
        if not res_length_bytes:
            raise RuntimeError("Process closed stdout")
        res_length = struct.unpack('>I', res_length_bytes)[0]
        
        res_payload = process.stdout.read(res_length)
        return msgpack.unpackb(res_payload, raw=False)

    def test_pulse_to_symbols_flow(self, slave_process):
        request = {
            "command": "parse",
            "path": "sample.py",
            "content": "def test_func():\n    return 42"
        }
        
        response = self._send_receive(slave_process, request)
        
        assert response["status"] == "ok"
        symbols = response["data"]["symbols"]
        assert len(symbols) == 1
        assert symbols[0]["name"] == "test_func"
        
    def test_healthcheck_resilience(self, slave_process):
        request = {"command": "ping"}
        response = self._send_receive(slave_process, request)
        
        assert response["status"] == "ok"
        assert response["data"] == "pong"

    def test_batch_parsing_performance(self, slave_process):
        batch_request = {
            "command": "parse_batch",
            "files": [
                {"path": f"file_{i}.py", "content": f"def func_{i}(): pass"}
                for i in range(10)
            ]
        }
        
        response = self._send_receive(slave_process, batch_request)
        
        assert response["status"] == "ok"
        assert len(response["data"]) == 10
        assert response["data"][0]["symbols"][0]["name"] == "func_0"
