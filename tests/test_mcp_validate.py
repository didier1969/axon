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
    def test_build_args_covers_public_operator_surface(self) -> None:
        self.assertEqual(
            MODULE.build_args("status", {}, "AXO", "axon", "checkout", {})["mode"],
            "brief",
        )
        project_status_args = MODULE.build_args("project_status", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(project_status_args["project_code"], "AXO")
        self.assertEqual(project_status_args["mode"], "brief")
        snapshot_history_args = MODULE.build_args("snapshot_history", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(snapshot_history_args["project_code"], "AXO")
        self.assertEqual(snapshot_history_args["limit"], 5)
        snapshot_diff_args = MODULE.build_args("snapshot_diff", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(snapshot_diff_args["project_code"], "AXO")
        conception_view_args = MODULE.build_args("conception_view", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(conception_view_args["project_code"], "AXO")
        change_safety_args = MODULE.build_args("change_safety", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(change_safety_args["target"], "checkout")
        self.assertEqual(change_safety_args["target_type"], "symbol")
        why_args = MODULE.build_args("why", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(why_args["symbol"], "checkout")
        self.assertEqual(why_args["project"], "AXO")
        path_args = MODULE.build_args("path", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(path_args["source"], "checkout")
        self.assertEqual(path_args["project"], "AXO")
        anomalies_args = MODULE.build_args("anomalies", {}, "AXO", "axon", "checkout", {})
        self.assertEqual(anomalies_args["project"], "AXO")
        self.assertEqual(anomalies_args["mode"], "brief")

    def test_build_args_reuses_preview_id_from_validation_state(self) -> None:
        args = MODULE.build_args(
            "soll_commit_revision",
            {},
            "BookingSystem",
            "booking",
            "Bookings",
            {"preview_id": "PRV-AXO-123"},
        )

        self.assertEqual(args["preview_id"], "PRV-AXO-123")

    def test_update_validation_state_tracks_latest_soll_export_path(self) -> None:
        original_latest = MODULE.latest_soll_export_path
        try:
            MODULE.latest_soll_export_path = lambda: "docs/vision/SOLL_EXPORT_TEST.md"
            state = {}
            MODULE.update_validation_state(state, "soll_export", {}, {"result": {"data": {}}})
        finally:
            MODULE.latest_soll_export_path = original_latest

        self.assertEqual(state["latest_soll_export_path"], "docs/vision/SOLL_EXPORT_TEST.md")

    def test_evaluate_tool_result_passes_non_mutating_ok_response(self) -> None:
        resp = {"result": {"content": [{"type": "text", "text": "ok"}]}}

        status, note = MODULE.evaluate_tool_result("query", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_checks_public_status_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "runtime_mode": "mcp_http",
                    "runtime_profile": "advanced",
                    "truth_status": "canonical",
                    "canonical_sources": {
                        "soll_export": {"reimportable": True},
                    },
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("status", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_checks_project_status_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "project_code": "AXO",
                    "snapshot_id": "project-status-1",
                    "generated_at": 123456789,
                    "delta_vs_previous": {"available": False},
                    "vision": {"id": "VIS-AXO-001"},
                    "runtime": {"runtime_mode": "mcp_http"},
                    "anomalies": {"summary": {}},
                    "conception": {"module_count": 1},
                    "soll_context": {"visions": []},
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("project_status", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_checks_snapshot_history_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "snapshots": [],
                    "storage": {"scope": "derived_non_canonical"},
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("snapshot_history", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_checks_snapshot_diff_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "from_snapshot_id": "snap-1",
                    "to_snapshot_id": "snap-2",
                    "metric_delta": {"orphan_code_count_delta": 1},
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("snapshot_diff", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_checks_conception_view_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "modules": [],
                    "interfaces": [],
                    "contracts": [],
                    "flows": [],
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("conception_view", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_checks_change_safety_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "target": "checkout",
                    "change_safety": "caution",
                    "coverage_signals": {},
                    "traceability_signals": {},
                    "validation_signals": {},
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("change_safety", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_rejects_incomplete_public_why_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {"framework_alias": "why"},
            }
        }

        status, note = MODULE.evaluate_tool_result("why", resp, "http://example.test", 1)

        self.assertEqual(status, "fail")
        self.assertIn("structured why payload", note)

    def test_evaluate_tool_result_checks_public_anomalies_contract(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "summary": {"wrapper_count": 1},
                    "findings": [],
                    "recommendations": [],
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("anomalies", resp, "http://example.test", 1)

        self.assertEqual(status, "ok")
        self.assertEqual(note, "ok")

    def test_evaluate_tool_result_checks_soll_query_context_includes_visions(self) -> None:
        resp = {
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "data": {
                    "project_code": "AXO",
                    "visions": ["VIS-AXO-001|Axon Vision|accepted|Vision first"],
                    "requirements": [],
                    "decisions": [],
                    "revisions": [],
                },
            }
        }

        status, note = MODULE.evaluate_tool_result("soll_query_context", resp, "http://example.test", 1)

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
