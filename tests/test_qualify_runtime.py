import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "qualify_runtime.py"
SPEC = importlib.util.spec_from_file_location("qualify_runtime", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class QualifyRuntimeTests(unittest.TestCase):
    def test_parse_args_defaults_to_demo_graph_only(self) -> None:
        args = MODULE.parse_args([])

        self.assertEqual(args.profile, "demo")
        self.assertEqual(args.mode, "graph_only")
        self.assertEqual(args.compare, "")
        self.assertEqual(args.duration, 60)
        self.assertEqual(args.output_root, str(MODULE.RUNS_ROOT))

    def test_plan_profile_steps_for_full(self) -> None:
        self.assertEqual(
            MODULE.profile_steps("full"),
            ["runtime_smoke", "mcp_validate", "mcp_robustness", "ingestion_qualify"],
        )

    def test_normalize_modes_prefers_compare_list(self) -> None:
        modes = MODULE.normalize_modes("graph_only", "mcp_only, graph_only ,full")

        self.assertEqual(modes, ["mcp_only", "graph_only", "full"])

    def test_overall_verdict_is_warn_when_any_step_warns(self) -> None:
        verdict = MODULE.combine_step_statuses(
            [
                {"name": "runtime_smoke", "status": "pass"},
                {"name": "mcp_validate", "status": "pass"},
                {"name": "mcp_robustness", "status": "warn"},
            ]
        )

        self.assertEqual(verdict, "warn")
        self.assertEqual(MODULE.exit_code_for_verdict(verdict), 1)

    def test_overall_verdict_is_fail_when_any_step_fails(self) -> None:
        verdict = MODULE.combine_step_statuses(
            [
                {"name": "runtime_smoke", "status": "pass"},
                {"name": "mcp_validate", "status": "fail"},
            ]
        )

        self.assertEqual(verdict, "fail")
        self.assertEqual(MODULE.exit_code_for_verdict(verdict), 2)

    def test_build_mode_comparison_uses_robustness_metrics(self) -> None:
        comparison = MODULE.build_mode_comparison(
            [
                {
                    "mode": "mcp_only",
                    "steps": {
                        "mcp_robustness": {
                            "status": "pass",
                            "summary": {
                                "modes": [
                                    {
                                        "mode": "mcp_only",
                                        "rates": {"responsive": 1.0, "success": 1.0},
                                        "latency_ms": {"p95": 300},
                                        "totals": {"timeout": 0, "backend_unavailable": 0},
                                    }
                                ]
                            },
                        }
                    },
                },
                {
                    "mode": "graph_only",
                    "steps": {
                        "mcp_robustness": {
                            "status": "pass",
                            "summary": {
                                "modes": [
                                    {
                                        "mode": "graph_only",
                                        "rates": {"responsive": 0.98, "success": 0.97},
                                        "latency_ms": {"p95": 420},
                                        "totals": {"timeout": 1, "backend_unavailable": 0},
                                    }
                                ]
                            },
                        }
                    },
                },
            ]
        )

        self.assertEqual(comparison["baseline"], "mcp_only")
        self.assertEqual(len(comparison["comparisons"]), 1)
        delta = comparison["comparisons"][0]
        self.assertEqual(delta["candidate_mode"], "graph_only")
        self.assertEqual(delta["responsive_rate_delta"], -0.02)
        self.assertEqual(delta["success_rate_delta"], -0.03)
        self.assertEqual(delta["p95_latency_ms_delta"], 120)
        self.assertEqual(delta["timeout_delta"], 1)
        self.assertEqual(delta["backend_unavailable_delta"], 0)


if __name__ == "__main__":
    unittest.main()
