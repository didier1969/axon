import pytest
# Ces imports échoueront tant que le code n'est pas écrit
# Mais ils définissent la structure micro attendue.
try:
    from axon.bridge.worker import AxonWorker
except ImportError:
    AxonWorker = None

"""
Tests Micro (Unitaires) pour l'esclave de parsing (Pod B).
Objectif : Valider la logique de transformation sans dépendre de l'OS ou du Pipe.
"""

@pytest.mark.skipif(AxonWorker is None, reason="AxonWorker non implémenté")
class TestAxonWorkerMicro:

    def test_worker_handles_ping(self):
        """Le worker doit répondre à un ping pour le healthcheck."""
        worker = AxonWorker()
        command = {"command": "ping"}
        response = worker.handle_command(command)
        
        assert response["status"] == "ok"
        assert response["data"] == "pong"

    def test_worker_handles_parse_valid_python(self):
        """Le worker doit transformer du code Python en symboles Axon."""
        worker = AxonWorker()
        code = "def hello():\n    return 'world'"
        command = {
            "command": "parse",
            "path": "test.py",
            "content": code
        }
        
        response = worker.handle_command(command)
        
        assert response["status"] == "ok"
        symbols = response["data"]["symbols"]
        assert any(s["name"] == "hello" and s["kind"] == "function" for s in symbols)

    def test_worker_handles_unsupported_language(self):
        """Le worker doit utiliser le TextParser en fallback pour l'inconnu."""
        worker = AxonWorker()
        command = {
            "command": "parse",
            "path": "config.unknown",
            "content": "key=value"
        }
        
        response = worker.handle_command(command)
        
        # Le contrat v1.0 stipule que même l'inconnu renvoie un status 'ok' avec un fallback
        assert response["status"] == "ok"
        assert len(response["data"]["symbols"]) == 1
        assert response["data"]["symbols"][0]["name"] == "config.unknown"

    def test_worker_error_encapsulation(self):
        """Le worker ne doit jamais crasher, il doit encapsuler l'erreur."""
        worker = AxonWorker()
        # Commande malformée (manque 'content')
        command = {"command": "parse", "path": "test.py"}
        
        response = worker.handle_command(command)
        
        assert response["status"] == "error"
        assert "message" in response

    def test_worker_handles_parse_batch(self):
        """Le worker doit traiter une liste de fichiers en une seule commande."""
        worker = AxonWorker()
        command = {
            "command": "parse_batch",
            "files": [
                {"path": "a.py", "content": "def a(): pass"},
                {"path": "b.py", "content": "def b(): pass"}
            ]
        }
        
        response = worker.handle_command(command)
        
        assert response["status"] == "ok"
        assert len(response["data"]) == 2
        assert response["data"][0]["symbols"][0]["name"] == "a"
        assert response["data"][1]["symbols"][0]["name"] == "b"
