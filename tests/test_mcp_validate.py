import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "mcp_validate.py"
SPEC = importlib.util.spec_from_file_location("mcp_validate", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class McpValidateTests(unittest.TestCase):
    def test_evaluate_tool_result_passes_non_mutating_ok_response(self) -> None:
        resp = {"result": {"content": [{"type": "text", "text": "ok"}]}}

        status, note = MODULE.evaluate_tool_result("query", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_requires_job_id_for_mutation(self) -> None:
        resp = {"result": {"data": {"accepted": True}}}

        status, note = MODULE.evaluate_tool_result(
            "soll_apply_plan", resp, "http://example.test", 1
        )

        self.assertEqual(status, "fail")
        self.assertIn("job_id", note)

    def test_evaluate_tool_result_polls_job_status_until_succeeded(self) -> None:
        calls = []
        responses = iter(
            [
                {"result": {"data": {"status": "queued", "error_text": ""}}},
                {"result": {"data": {"status": "running", "error_text": ""}}},
                {"result": {"data": {"status": "succeeded", "error_text": ""}}},
            ]
        )

        def fake_rpc_call(url, payload, timeout):
            calls.append({"url": url, "payload": payload, "timeout": timeout})
            return next(responses)

        original_rpc_call = MODULE.rpc_call
        original_sleep = MODULE.time.sleep
        try:
            MODULE.rpc_call = fake_rpc_call
            MODULE.time.sleep = lambda _: None
            resp = {"result": {"data": {"accepted": True, "job_id": "JOB-123"}}}
            status, note = MODULE.evaluate_tool_result(
                "soll_manager", resp, "http://example.test", 2
            )
        finally:
            MODULE.rpc_call = original_rpc_call
            MODULE.time.sleep = original_sleep

        self.assertEqual(status, "ok")
        self.assertIn("JOB-123", note)
        self.assertEqual(len(calls), 3)
        self.assertEqual(calls[0]["payload"]["params"]["name"], "job_status")
        self.assertEqual(calls[0]["payload"]["params"]["arguments"]["job_id"], "JOB-123")

    def test_evaluate_tool_result_warns_when_job_finishes_failed(self) -> None:
        def fake_rpc_call(url, payload, timeout):
            return {
                "result": {
                    "data": {
                        "status": "failed",
                        "error_text": "synthetic validation failure",
                    }
                }
            }

        original_rpc_call = MODULE.rpc_call
        original_sleep = MODULE.time.sleep
        try:
            MODULE.rpc_call = fake_rpc_call
            MODULE.time.sleep = lambda _: None
            resp = {"result": {"data": {"accepted": True, "job_id": "JOB-456"}}}
            status, note = MODULE.evaluate_tool_result(
                "soll_commit_revision", resp, "http://example.test", 2
            )
        finally:
            MODULE.rpc_call = original_rpc_call
            MODULE.time.sleep = original_sleep

        self.assertEqual(status, "warn")
        self.assertIn("JOB-456", note)
        self.assertIn("synthetic validation failure", note)


if __name__ == "__main__":
    unittest.main()
