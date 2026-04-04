import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "qualify_mcp_robustness.py"
SPEC = importlib.util.spec_from_file_location("qualify_mcp_robustness", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def make_event(
    *,
    category: str,
    responded: bool,
    duration_ms: int,
    started_at_ms: int,
    worker: int = 0,
    request: str = "health",
    excerpt: str = "",
) -> dict:
    return {
        "worker": worker,
        "request": request,
        "category": category,
        "responded": responded,
        "duration_ms": duration_ms,
        "started_at_ms": started_at_ms,
        "excerpt": excerpt,
    }


class QualifyMcpRobustnessTests(unittest.TestCase):
    def setUp(self) -> None:
        self.thresholds = {
            "responsive_rate_warn": 0.99,
            "responsive_rate_degraded": 0.95,
            "max_timeouts_warn": 0,
            "max_timeouts_degraded": 2,
            "max_jsonrpc_errors_degraded": 0,
            "p95_latency_warn_ms": 1500,
        }

    def test_summarize_events_marks_pass_without_failures(self) -> None:
        events = [
            make_event(category="ok_result", responded=True, duration_ms=40, started_at_ms=1000),
            make_event(category="ok_result", responded=True, duration_ms=55, started_at_ms=1100),
            make_event(category="app_error", responded=True, duration_ms=65, started_at_ms=1200),
        ]

        summary = MODULE.summarize_events("mcp_only", events, self.thresholds)

        self.assertEqual(summary["verdict"], "pass")
        self.assertEqual(summary["totals"]["requests"], 3)
        self.assertEqual(summary["totals"]["responded"], 3)
        self.assertEqual(summary["totals"]["ok_result"], 2)
        self.assertEqual(summary["totals"]["app_error"], 1)
        self.assertEqual(summary["rates"]["responsive"], 1.0)
        self.assertFalse(summary["resilience"]["ever_failed"])
        self.assertFalse(summary["resilience"]["recovered_without_restart"])

    def test_summarize_events_detects_recovery_after_timeout(self) -> None:
        thresholds = dict(self.thresholds)
        thresholds["responsive_rate_degraded"] = 0.5
        thresholds["max_timeouts_degraded"] = 5
        events = [
            make_event(category="ok_result", responded=True, duration_ms=50, started_at_ms=1000),
            make_event(category="timeout", responded=False, duration_ms=5000, started_at_ms=2000),
            make_event(category="ok_result", responded=True, duration_ms=45, started_at_ms=7010),
            make_event(category="ok_result", responded=True, duration_ms=50, started_at_ms=7100),
        ]

        summary = MODULE.summarize_events("full", events, thresholds)

        self.assertEqual(summary["verdict"], "warn")
        self.assertTrue(summary["resilience"]["ever_failed"])
        self.assertTrue(summary["resilience"]["recovered_without_restart"])
        self.assertEqual(summary["resilience"]["recovery_time_ms"], 5010)
        self.assertEqual(len(summary["failure_samples"]), 1)
        self.assertEqual(summary["failure_samples"][0]["category"], "timeout")

    def test_summarize_events_marks_degraded_when_timeouts_exceed_threshold(self) -> None:
        events = [
            make_event(category="timeout", responded=False, duration_ms=5000, started_at_ms=1000, worker=0),
            make_event(category="timeout", responded=False, duration_ms=5000, started_at_ms=2000, worker=1),
            make_event(category="timeout", responded=False, duration_ms=5000, started_at_ms=3000, worker=0),
            make_event(category="ok_result", responded=True, duration_ms=40, started_at_ms=8100),
        ]

        summary = MODULE.summarize_events("full", events, self.thresholds)

        self.assertEqual(summary["verdict"], "degraded")
        self.assertEqual(summary["totals"]["timeout"], 3)
        self.assertEqual(summary["rates"]["responsive"], 0.25)

    def test_compare_modes_reports_expected_deltas(self) -> None:
        baseline = {
            "mode": "mcp_only",
            "rates": {"responsive": 1.0, "success": 1.0},
            "latency_ms": {"p95": 300},
            "totals": {"timeout": 0, "backend_unavailable": 0},
            "verdict": "pass",
        }
        candidate = {
            "mode": "full",
            "rates": {"responsive": 0.84, "success": 0.84},
            "latency_ms": {"p95": 640},
            "totals": {"timeout": 4, "backend_unavailable": 1},
            "verdict": "degraded",
        }

        comparison = MODULE.compare_modes([baseline, candidate])

        self.assertEqual(comparison["baseline"], "mcp_only")
        self.assertEqual(len(comparison["comparisons"]), 1)
        delta = comparison["comparisons"][0]
        self.assertEqual(delta["candidate_mode"], "full")
        self.assertEqual(delta["responsive_rate_delta"], -0.16)
        self.assertEqual(delta["success_rate_delta"], -0.16)
        self.assertEqual(delta["p95_latency_ms_delta"], 340)
        self.assertEqual(delta["timeout_delta"], 4)
        self.assertEqual(delta["backend_unavailable_delta"], 1)
        self.assertEqual(delta["verdict"], "degraded")


if __name__ == "__main__":
    unittest.main()
