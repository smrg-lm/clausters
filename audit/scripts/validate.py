#!/usr/bin/env python3
"""Validate operational audit resources without running application workloads."""
import json
import re
from pathlib import Path
import sys

from audit import validate_scenario

ROOT = Path(__file__).resolve().parents[1]


def main():
    try:
        required = ("README.md", "scope.md", "methodology.md", "formats.md", "PLAN.md", "development.md",
                    "scripts/audit.py", "scripts/sample_process.py", "tests/test_harness.py", "fixtures/generate.py",
                    "tools/package-setup.md", "tools/python-requirements.txt", "templates/manual-evidence.json",
                    "procedures/audio.md", "procedures/gui.md", "procedures/combined.md", "procedures/crates.md",
                    "procedures/clients.md", "procedures/editor.md", "procedures/review.md")
        for name in required:
            if not (ROOT / name).is_file():
                raise ValueError(f"Missing resource: {name}")
        for path in ROOT.rglob("*.md"):
            for target in re.findall(r"\]\(([^)]+)\)", path.read_text()):
                if "://" not in target and not (path.parent / target.split("#")[0]).exists():
                    raise ValueError(f"Broken local link: {path}: {target}")
        for name in ("audio", "gui", "combined", "workspace", "python", "web", "editor"):
            value = json.loads((ROOT / "scenarios" / (name + ".json")).read_text())
            validate_scenario(value)
            if value["id"] != name:
                raise ValueError("Scenario filename and identity differ")
        profile = json.loads((ROOT / "tools/versions.json").read_text())
        if profile["status"] != "pinned" or not profile["selected_tools"]:
            raise ValueError("Tool profile must be pinned")
        for path in (ROOT / "templates/record").glob("*.json"):
            if json.loads(path.read_text())["status"] != "template":
                raise ValueError("Record templates must remain templates")
        print("Audit resources valid; application workloads were not executed.")
        return 0
    except (OSError, ValueError, KeyError) as exc:
        print(exc, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
