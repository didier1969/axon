import subprocess
import json
import os
import tempfile
import pathlib

def test_elixir_extractor():
    # Dynamically resolve script path relative to this file
    script_dir = pathlib.Path(__file__).parent.parent.resolve()
    extractor_script = script_dir / "scripts" / "extract_elixir_deps.exs"

    # Use a secure temporary directory that auto-cleans
    with tempfile.TemporaryDirectory() as tmpdirname:
        project_dir = os.path.join(tmpdirname, "child_a")
        os.makedirs(project_dir)
        
        mix_exs_path = os.path.join(project_dir, "mix.exs")
        with open(mix_exs_path, "w") as f:
            f.write("""
            defmodule ChildA.MixProject do
              use Mix.Project
              def project do
                [app: :child_a, deps: [{:child_b, in_umbrella: true}, {:external, path: "../external"}]]
              end
            end
            """)
        
        result = subprocess.run(
            ["elixir", str(extractor_script), project_dir], 
            capture_output=True, text=True
        )
        
        assert result.returncode == 0, f"Script failed: {result.stderr}"
        
        try:
            data = json.loads(result.stdout)
        except json.JSONDecodeError as e:
            assert False, f"Failed to parse JSON: {result.stdout}\nError: {e}"
            
        assert data["node"] == "child_a"
        assert len(data["edges"]) == 2
        
        # Verify the structure and escaping
        targets = [e["to"] for e in data["edges"]]
        assert "child_b" in targets
        assert "external" in targets

if __name__ == "__main__":
    test_elixir_extractor()
    print("✅ test_elixir_extractor passed.")
