"""Harness contract tests: synthetic repositories and measurements, never Clausters."""

from contextlib import redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from unittest.mock import patch

AUDIT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("audit_runner", AUDIT / "scripts/audit.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
fixture_spec = importlib.util.spec_from_file_location("fixtures", AUDIT / "fixtures/generate.py")
fixtures = importlib.util.module_from_spec(fixture_spec)
fixture_spec.loader.exec_module(fixtures)


class HarnessTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="audit-harness-test-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Audit harness test")
        self.git("config", "user.email", "test@example.invalid")
        (self.repo / "Cargo.toml").write_text('[package]\nname = "synthetic"\nversion = "0.0.0"\n')
        (self.repo / "tracked.txt").write_text("base\n")
        (self.repo / "delete.txt").write_text("remove\n")
        (self.repo / ".gitignore").write_text("ignored/\ntarget\n")
        self.git("add", ".")
        self.git("commit", "-m", "Synthetic fixture")
        self.base = self.git("rev-parse", "HEAD")
        # The harness checks the reviewed tool profile; these tests check the
        # harness, so they run against the versions this machine has.
        profile = self.root / "versions.json"
        selected = [{"name": name, "command": argv, "version_output": audit.command(argv)}
                    for name, argv in (("python", ["python3", "--version"]),
                                       ("git", ["git", "--version"]))]
        audit.write(profile, {"format_version": 1, "status": "pinned",
                              "profile": "synthetic", "selected_tools": selected})
        tool_profile = patch.object(audit, "TOOL_PROFILE", profile)
        tool_profile.start()
        self.addCleanup(tool_profile.stop)

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *argv):
        return audit.command(["git", *argv], self.repo)

    def scenario(self, manual=False):
        value = {"format_version": 1, "id": "rehearsal", "status": "ready", "steps": [
            {"id": "known-data", "kind": "command", "required": True,
             "argv": [sys.executable, "-c", 'print(\'{"name":"known","unit":"count","value":4,"direction":"higher"}\')'],
             "cwd": ".", "repetitions": 2, "warmup": 1,
             "timeout_seconds": 10, "parser": "metrics"}]}
        if manual:
            value["steps"].append({"id": "operator", "kind": "manual", "required": True,
                                   "instructions": "Synthetic manual-evidence contract"})
        return value

    def prepare(self):
        work = self.root / "work"
        with patch.object(audit, "REPO", self.repo), redirect_stdout(io.StringIO()):
            audit.prepare(work, "HEAD")
        return work

    def run_known(self, work, name, manual=False):
        path = self.root / "scenario.json"
        audit.write(path, self.scenario(manual))
        with redirect_stdout(io.StringIO()):
            self.assertEqual(audit.run_campaign(work, path, name), 3 if manual else 0)
        return work / "runs" / name

    def test_rehearsal_prepare_run_manual_seal_archive_compare(self):
        work = self.prepare()
        self.assertTrue((work / "tools/python/bin/python").exists())
        self.assertEqual(self.git("status", "--porcelain"), "")
        run = self.run_known(work, "first", manual=True)
        self.assertEqual(audit.read(run / "manifest.json")["status"], "partial")
        with self.assertRaises(ValueError):
            audit.seal_record(run, "test")
        artifact = self.root / "proof.txt"
        artifact.write_text("Synthetic contract evidence, not application audit results.\n")
        payload = self.root / "manual.json"
        audit.write(payload, {"status": "passed", "operator": "harness-test", "procedure": "synthetic",
                              "configuration": "no devices", "limitations": "not application evidence",
                              "reason": "attachment contract", "artifacts": [str(artifact)]})
        audit.attach(run, "operator", payload)
        audit.seal_record(run, "harness-test")
        self.assertTrue(audit.verify_record(run))
        summary = audit.read(run / "results/summary.json")
        known = next(row for row in summary if row["name"] == "known")
        self.assertEqual((known["count"], known["median"], known["p95"]), (2, 4, 4))
        with self.assertRaises(ValueError):
            audit.attach(run, "operator", payload)
        output = self.root / "record.tar.gz"
        audit.archive(run, output)
        self.assertEqual(audit.read(str(output) + ".json")["sha256"], audit.digest(output))
        with tarfile.open(output) as archive:
            self.assertIn("record/manifest.json", archive.getnames())
            self.assertIn("inputs/source.tar", archive.getnames())
            self.assertIn("inputs/resources.tar", archive.getnames())
        self.assertFalse(audit.compare(run, run, .1)["regression"])
        (run / "findings.md").write_text("tamper")
        with self.assertRaises(ValueError):
            audit.verify_record(run)

    def test_patch_includes_staged_unstaged_new_binary_and_deletion_without_index_mutation(self):
        (self.repo / "tracked.txt").write_text("staged\n")
        self.git("add", "tracked.txt")
        (self.repo / "tracked.txt").write_text("unstaged after staging\n")
        (self.repo / "delete.txt").unlink()
        (self.repo / "new.txt").write_text("new\n")
        (self.repo / "new.bin").write_bytes(bytes(range(256)))
        (self.repo / "ignored").mkdir()
        (self.repo / "ignored/intentional.txt").write_text("explicitly staged ignored input\n")
        self.git("add", "-f", "ignored/intentional.txt")
        original_index = (self.repo / ".git/index").read_bytes()
        output = self.root / "instrumentation.patch"
        audit.export_patch(self.repo, self.base, output)
        self.assertEqual((self.repo / ".git/index").read_bytes(), original_index)
        result = audit.replay(self.repo, self.base, output)
        self.assertTrue(result["replay_verified"])
        self.assertIn(b"GIT binary patch", output.read_bytes())
        self.assertIn(b"new.txt", output.read_bytes())
        self.assertIn(b"ignored/intentional.txt", output.read_bytes())
        self.assertIn(b"delete.txt", output.read_bytes())
        output.write_bytes(b"invalid patch")
        with self.assertRaises(subprocess.SubprocessError):
            audit.replay(self.repo, self.base, output)

    def test_instrumented_run_is_separate_and_replayed(self):
        work = self.prepare()
        (self.repo / "tracked.txt").write_text("instrumentation\n")
        output = self.root / "instrumentation.patch"
        audit.export_patch(self.repo, self.base, output)
        path = self.root / "scenario.json"
        audit.write(path, self.scenario())
        with redirect_stdout(io.StringIO()):
            audit.run_campaign(work, path, "patched", output)
        run = work / "runs/patched"
        self.assertTrue(audit.read(run / "manifest.json")["instrumentation"]["replay_verified"])
        self.assertEqual((work / "source/tracked.txt").read_text(), "base\n")
        self.assertEqual((work / "instrumented/patched/tracked.txt").read_text(), "instrumentation\n")
        audit.seal_record(run, "test")

    def test_parser_rejects_empty_tests_empty_metrics_and_nonfinite_values(self):
        for parser, output in (("rust-tests", "test result: ok. 0 passed"),
                               ("bench", ""), ("metrics", ""),
                               ("metrics", '{"name":"bad","unit":"s","value":NaN}')):
            with self.assertRaises(ValueError):
                audit.parse_metrics(parser, output)
        rows = audit.parse_metrics("bench", '{"name":"fft","peak_block":50,"gated":true}')
        self.assertEqual(rows[0]["unit"], "microseconds")
        for output in ("# pass 3\n# skipped 1", "\u2139 pass 3\n\u2139 skipped 1"):
            rows = audit.parse_metrics("node-tests", output)
            self.assertEqual([row["value"] for row in rows], [3, 1])

    def test_subprocess_timeout_and_nonzero_exit(self):
        for code, timeout, expected in (("raise SystemExit(7)", 10, "failed"),
                                        ("import time; time.sleep(5)", .03, "error")):
            result = audit.execute([sys.executable, "-c", code], self.root, os.environ.copy(), timeout,
                                   self.root / "out", self.root / "err")
            self.assertEqual(result["status"], expected)

    def test_failure_record_is_retained_and_cannot_pass(self):
        work = self.prepare()
        scenario = self.scenario()
        scenario["steps"][0]["argv"] = [sys.executable, "-c", "raise SystemExit(2)"]
        path = self.root / "scenario.json"
        audit.write(path, scenario)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(audit.run_campaign(work, path, "failed"), 1)
        run = work / "runs/failed"
        self.assertEqual(audit.read(run / "manifest.json")["status"], "failed")
        with self.assertRaises(ValueError):
            audit.seal_record(run, "test")
        audit.seal_record(run, "test", preserve_incomplete=True, reason="Synthetic failure evidence retained")
        self.assertEqual(audit.read(run / "manifest.json")["status"], "failed")
        self.assertFalse(audit.read(run / "manifest.json")["quality_accepted"])
        self.assertTrue(audit.verify_record(run))
        audit.archive(run, self.root / "failed-evidence.tar.gz")

    def test_compare_rejects_incompatible_evidence(self):
        work = self.prepare()
        first, second = self.run_known(work, "one"), self.run_known(work, "two")
        # Set only the descriptive command-wall timings equal to isolate the known metric.
        rows = audit.load_rows(second)
        for row in rows:
            if row["name"] == "known":
                row["value"] = 2
        (second / "results/measurements.jsonl").write_text("")
        audit.append_rows(second, rows)
        audit.refresh(second)
        audit.seal_record(first, "test")
        audit.seal_record(second, "test")
        self.assertTrue(audit.compare(first, second, .1)["regression"])
        # Comparability changes before sealing require separate evidence sets.
        third = self.run_known(work, "three")
        value = audit.read(third / "manifest.json")
        value["compatibility"]["governor"] = "different"
        audit.write(third / "manifest.json", value)
        audit.seal_record(third, "test")
        with self.assertRaises(ValueError):
            audit.compare(first, third, .1)

    def test_paths_and_draft_scenarios_are_rejected(self):
        with self.assertRaises(ValueError):
            audit.safe_child(self.root, "../escape")
        with self.assertRaises(ValueError):
            audit.external(self.repo / "nested")
        scenario = self.scenario()
        scenario["status"] = "draft"
        with self.assertRaises(ValueError):
            audit.validate_scenario(scenario)

    def test_fixtures_are_deterministic_and_hashes_match(self):
        first = fixtures.generate(self.root / "fixture-one")
        second = fixtures.generate(self.root / "fixture-two")
        self.assertEqual(first, second)
        for row in first:
            self.assertEqual(row["sha256"], audit.digest(self.root / "fixture-one" / row["path"]))

    def test_inventory_keeps_editor_separate_and_new_crates_visible(self):
        for directory, name in (("crates/new-core", "new-core"), ("clausters-editor/src-tauri", "clausters-editor")):
            path = self.repo / directory / "Cargo.toml"
            path.parent.mkdir(parents=True)
            path.write_text(f'[package]\nname = "{name}"\nversion = "0.0.0"\n'.replace("\\n", "\n"))
        package = self.repo / "clients/python/pyproject.toml"
        package.parent.mkdir(parents=True)
        package.write_text('[project]\nname = "clausters"\nversion = "0.0.0"\n'.replace("\\n", "\n"))
        self.git("add", ".")
        inventory = audit.crate_inventory(self.repo)
        self.assertEqual(len(inventory), 3)
        editor = next(row for row in inventory if row["package"] == "clausters-editor")
        self.assertEqual(editor["unit"], "editor")
        self.assertEqual(audit.package_inventory(self.repo)[0]["unit"], "python")

    def test_missing_crate_coverage_cannot_be_attached_as_pass(self):
        work = self.prepare()
        scenario = self.scenario(manual=True)
        scenario["steps"][-1]["id"] = "crate-review"
        path = self.root / "scenario.json"
        audit.write(path, scenario)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(audit.run_campaign(work, path, "coverage"), 3)
        artifact = self.root / "review.md"
        artifact.write_text("Synthetic review, not an application audit.")
        payload = {"status": "passed", "operator": "test", "procedure": "test", "configuration": "test",
                   "limitations": "synthetic", "reason": "test", "artifacts": [str(artifact)]}
        evidence = self.root / "evidence.json"
        audit.write(evidence, payload)
        run = work / "runs/coverage"
        with self.assertRaises(ValueError):
            audit.attach(run, "crate-review", evidence)
        payload["crate_coverage"] = {"Cargo.toml": {"status": "passed", "reason": "synthetic", "evidence": "review.md"}}
        audit.write(evidence, payload)
        audit.attach(run, "crate-review", evidence)
        audit.seal_record(run, "test")

    def test_client_test_skip_is_partial_and_blocks_acceptance(self):
        work = self.prepare()
        scenario = self.scenario()
        scenario["steps"][0].update({"parser": "pytest", "argv": [sys.executable, "-c", "print('3 passed, 1 skipped')"]})
        path = self.root / "scenario.json"
        audit.write(path, scenario)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(audit.run_campaign(work, path, "skips"), 3)
        with self.assertRaises(ValueError):
            audit.seal_record(work / "runs/skips", "test")

    def test_offline_unit_setup_installs_only_into_external_venv(self):
        work = self.prepare()
        wheelhouse = self.root / "wheelhouse"
        wheelhouse.mkdir()
        wheel = wheelhouse / "audit_fixture-1.0.0-py3-none-any.whl"
        with zipfile.ZipFile(wheel, "w") as archive:
            archive.writestr("audit_fixture.py", "VALUE = 4\n")
            archive.writestr("audit_fixture-1.0.0.dist-info/METADATA", "Metadata-Version: 2.1\nName: audit-fixture\nVersion: 1.0.0\n")
            archive.writestr("audit_fixture-1.0.0.dist-info/WHEEL", "Wheel-Version: 1.0\nGenerator: audit-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n")
            archive.writestr("audit_fixture-1.0.0.dist-info/RECORD", "")
        # Synthetic installer fixture, not the real pytest requirements.
        (work / "resources/tools/python-requirements.txt").write_text("audit-fixture==1.0.0\n")
        with redirect_stdout(io.StringIO()):
            audit.setup_unit(work, "python", wheelhouse)
        result = audit.command([str(work / "tools/client-python/bin/python"), "-c", "import audit_fixture; print(audit_fixture.VALUE)"])
        self.assertEqual(result, "4")
        receipt = audit.read(work / "setup/python/receipt.json")
        self.assertTrue(all(row["status"] == "passed" for row in receipt))
        self.assertEqual(audit.read(work / "setup/python/wheelhouse.json")[wheel.name], audit.digest(wheel))
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_linux_process_sample_fields_and_nonfinite_arguments(self):
        spec = importlib.util.spec_from_file_location("sampler", AUDIT / "scripts/sample_process.py")
        sampler = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(sampler)
        values = ["0"] * 22
        values[0], values[11], values[12], values[19], values[21] = "R", "100", "50", "123", "10"
        sample = sampler.decode_stat("7 (name with ) parenthesis) " + " ".join(values), 100, 4096)
        self.assertEqual(sample, {"cpu_seconds": 1.5, "start_ticks": 123, "rss_bytes": 40960})
        with self.assertRaises(ValueError):
            sampler.sample(7, float("nan"), .1, self.root / "unused")


    def test_unexpected_manifest_change_blocks_measurement(self):
        work = self.prepare()
        scenario = self.scenario()
        scenario["steps"][0]["argv"] = [sys.executable, "-c", "from pathlib import Path; Path('Cargo.toml').write_text('changed')"]
        scenario["steps"][0]["parser"] = "exit"
        path = self.root / "scenario.json"
        audit.write(path, scenario)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(audit.run_campaign(work, path, "manifest-drift"), 1)
        checks = audit.read(work / "runs/manifest-drift/results/checks.json")
        self.assertEqual(checks[0]["status"], "error")
        self.assertIn("manifest/lockfile", checks[0]["reason"])


if __name__ == "__main__":
    unittest.main()
