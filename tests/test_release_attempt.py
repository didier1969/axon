import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[1]


def load_module(name: str, path: pathlib.Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec and spec.loader
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


durable_hook = load_module(
    "durable_hook", ROOT / "scripts" / "release" / "durable_hook.py"
)
create_manifest = load_module(
    "create_manifest", ROOT / "scripts" / "release" / "create_manifest.py"
)


class ReleaseAttemptTests(unittest.TestCase):
    def test_manifest_bin_dir_selects_frozen_candidate(self):
        with tempfile.TemporaryDirectory() as raw:
            bin_dir = pathlib.Path(raw)
            (bin_dir / "axon-brain").write_bytes(b"candidate")
            (bin_dir / "axon-brain.build-info").write_text("AXON_BUILD_ID=v-test\n")
            _, artifact, build_info = create_manifest.runtime_primary_artifact(
                ROOT, bin_dir, None, None
            )
            self.assertEqual(artifact, bin_dir / "axon-brain")
            self.assertEqual(build_info, bin_dir / "axon-brain.build-info")

    def test_release_attempt_id_prefers_explicit_then_environment(self):
        with mock.patch.dict("os.environ", {"AXON_RELEASE_ATTEMPT_ID": "attempt-from-env"}):
            self.assertEqual(
                create_manifest.resolve_release_attempt_id("attempt-explicit"),
                "attempt-explicit",
            )
            self.assertEqual(
                create_manifest.resolve_release_attempt_id(None), "attempt-from-env"
            )

    def test_durable_hook_retries_then_completes_with_atomic_evidence(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp_path = pathlib.Path(raw)
            counter = tmp_path / "counter"
            state_dir = tmp_path / "hooks"
            command = [
                sys.executable,
                "-c",
                (
                    "import pathlib,sys; p=pathlib.Path(sys.argv[1]); "
                    "n=int(p.read_text())+1 if p.exists() else 1; p.write_text(str(n)); "
                    "raise SystemExit(0 if n == 2 else 9)"
                ),
                str(counter),
            ]
            rc = durable_hook.run_hook(
                state_root=state_dir,
                attempt_id="attempt-123",
                hook_name="soll-export",
                command=command,
                max_attempts=3,
                timeout_seconds=5,
                retry_delay_seconds=0,
            )
            self.assertEqual(rc, 0)
            state = json.loads(
                (state_dir / "attempt-123" / "soll-export.json").read_text()
            )
            self.assertEqual(state["release_attempt_id"], "attempt-123")
            self.assertEqual(state["status"], "completed")
            self.assertEqual(state["attempts_made"], 2)
            self.assertEqual(
                [entry["status"] for entry in state["history"]],
                ["failed", "completed"],
            )
            self.assertFalse(list(state_dir.rglob("*.tmp-*")))

    def test_durable_hook_bounds_failure_and_preserves_distinct_status(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp_path = pathlib.Path(raw)
            rc = durable_hook.run_hook(
                state_root=tmp_path,
                attempt_id="attempt-failed",
                hook_name="projection",
                command=[sys.executable, "-c", "raise SystemExit(17)"],
                max_attempts=2,
                timeout_seconds=5,
                retry_delay_seconds=0,
            )
            self.assertEqual(rc, 17)
            state = json.loads(
                (tmp_path / "attempt-failed" / "projection.json").read_text()
            )
            self.assertEqual(state["status"], "failed")
            self.assertEqual(state["attempts_made"], 2)
            self.assertEqual(state["last_exit_code"], 17)


if __name__ == "__main__":
    unittest.main()
