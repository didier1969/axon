import importlib.util
import json
import tempfile
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
    def test_command_env_promotes_full_to_full_autonomous(self) -> None:
        env = MODULE.command_env("full")

        self.assertEqual(env["AXON_ENABLE_AUTONOMOUS_INGESTOR"], "true")
        self.assertEqual(env["AXON_RUNTIME_PROFILE"], "full_autonomous")

    def test_command_env_keeps_graph_only_neutral(self) -> None:
        env = MODULE.command_env("graph_only")

        self.assertNotIn("AXON_RUNTIME_PROFILE", env)
        self.assertNotIn("AXON_ENABLE_AUTONOMOUS_INGESTOR", env)

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
            ["runtime_smoke", "mcp_validate", "retrieval_qualify", "mcp_robustness", "ingestion_qualify"],
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

    def test_run_mcp_validate_keeps_mutations_disabled_by_default(self) -> None:
        captured: dict[str, object] = {}

        def fake_shell(cmd, *, check=False, env=None, timeout=None):
            captured["cmd"] = cmd
            captured["env"] = env
            captured["timeout"] = timeout
            return MODULE.subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        original_shell = MODULE.shell
        try:
            MODULE.shell = fake_shell
            args = MODULE.parse_args([])
            with tempfile.TemporaryDirectory() as tmpdir:
                run_dir = Path(tmpdir)
                summary_path = run_dir / "mcp_validate.json"
                summary_path.write_text(
                    json.dumps({"summary": {"ok": 3, "warn": 0, "fail": 0, "skip": 0}}),
                    encoding="utf-8",
                )
                result = MODULE.run_mcp_validate(args, "full", run_dir)
        finally:
            MODULE.shell = original_shell

        self.assertEqual(result["status"], "pass")
        self.assertNotIn("--allow-mutations", captured["cmd"])
        self.assertEqual(captured["env"]["AXON_RUNTIME_PROFILE"], "full_autonomous")
        self.assertEqual(
            captured["env"]["AXON_ENABLE_AUTONOMOUS_INGESTOR"], "true"
        )

    def test_run_runtime_smoke_skips_embedded_mcp_gate(self) -> None:
        calls: list[list[str]] = []

        def fake_shell(cmd, *, check=False, env=None, timeout=None):
            calls.append(cmd)
            return MODULE.subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        def fake_wait_for_mcp_ready(url, timeout_s):
            return None

        original_shell = MODULE.shell
        original_wait = MODULE.wait_for_mcp_ready
        try:
            MODULE.shell = fake_shell
            MODULE.wait_for_mcp_ready = fake_wait_for_mcp_ready
            with tempfile.TemporaryDirectory() as tmpdir:
                result = MODULE.run_runtime_smoke("full", Path(tmpdir), MODULE.MCP_URL)
        finally:
            MODULE.shell = original_shell
            MODULE.wait_for_mcp_ready = original_wait

        self.assertEqual(result["status"], "pass")
        self.assertEqual(calls[0], ["bash", "scripts/stop.sh"])
        self.assertEqual(
            calls[1],
            ["bash", "scripts/start.sh", "--full", "--skip-mcp-tests"],
        )

    def test_run_runtime_smoke_tolerates_start_timeout_when_runtime_becomes_ready(self) -> None:
        calls: list[list[str]] = []

        def fake_shell(cmd, *, check=False, env=None, timeout=None):
            calls.append(cmd)
            if cmd[:2] == ["bash", "scripts/start.sh"]:
                raise MODULE.subprocess.TimeoutExpired(cmd, timeout or 0, output="booting", stderr="")
            return MODULE.subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        def fake_wait_for_mcp_ready(url, timeout_s):
            return None

        original_shell = MODULE.shell
        original_wait = MODULE.wait_for_mcp_ready
        try:
            MODULE.shell = fake_shell
            MODULE.wait_for_mcp_ready = fake_wait_for_mcp_ready
            with tempfile.TemporaryDirectory() as tmpdir:
                result = MODULE.run_runtime_smoke("graph_only", Path(tmpdir), MODULE.MCP_URL)
        finally:
            MODULE.shell = original_shell
            MODULE.wait_for_mcp_ready = original_wait

        self.assertEqual(result["status"], "pass")
        self.assertIn("exceeded", result["note"])


if __name__ == "__main__":
    unittest.main()
