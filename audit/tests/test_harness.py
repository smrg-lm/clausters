"""Harness contract tests: synthetic repositories and measurements, never Clausters."""

from contextlib import redirect_stdout
import importlib.util
import io
import json
import os
import shutil
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
        # What git tracks under audit/, as `prepare` copies it: a checkout's
        # ignored records must not reach the fixture.
        names = subprocess.check_output(["git", "ls-files", "-z", "--", "."], cwd=AUDIT).decode().split("\0")
        for name in filter(None, names):
            destination = self.repo / "audit" / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(AUDIT / name, destination)
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

    def test_noise_reference_uses_sealed_records_and_matching_archives(self):
        import baseline
        work = self.prepare()
        scenario = self.scenario()
        scenario["steps"][0]["repetitions"] = 3
        path = self.root / "noise-scenario.json"
        audit.write(path, scenario)
        records, archives = [], []
        for name in ("baseline", "noise"):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(audit.run_campaign(work, path, name), 0)
            run = work / "runs" / name
            audit.seal_record(run, "synthetic-test")
            records.append(run)
            output = self.root / (name + ".tar.gz")
            audit.archive(run, output)
            archives.append(output)
        reference = baseline.establish(records, "synthetic-test", "Synthetic contract, not application acceptance")
        self.assertEqual(reference["tolerance"], 0)
        self.assertEqual(len(reference["metrics"]), 1)
        self.assertEqual(baseline.archive_identity(records[0], archives[0])["sha256"], audit.digest(archives[0]))
        (records[1] / "findings.md").write_text("tampered")
        with self.assertRaises(ValueError):
            baseline.establish(records, "synthetic-test", "reason")

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
            self.assertEqual([row["value"] for row in rows], [3, 1, 0])

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
        self.assertEqual(len(inventory), 4)
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
        payload["crate_coverage"] = {name: {"status": "passed", "reason": "synthetic", "evidence": "review.md"} for name in ("Cargo.toml", "audit/fuzz/Cargo.toml")}
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


    def test_tracked_resources_only_and_directory_ignore_targets(self):
        editor = self.repo / "clausters-editor/src-tauri"
        editor.mkdir(parents=True)
        (editor.parent / ".gitignore").write_text("/src-tauri/target/\n")
        (self.repo / ".gitignore").write_text("target\naudit/records/\naudit/fuzz/target/\n")
        self.git("add", ".")
        self.git("commit", "-m", "Real directory ignore contracts")
        ignored = self.repo / "audit/records/large.bin"
        ignored.parent.mkdir()
        ignored.write_bytes(b"x" * 100000)
        # The source repository owns the resource list, even when an ignored
        # resource exists beside tracked files in the runner's directory.
        with patch.object(audit, "AUDIT", self.repo / "audit"):
            work = self.prepare()
        self.assertFalse((work / "source/clausters-editor/src-tauri/target").exists())
        self.assertFalse((work / "resources/records").exists())
        with tarfile.open(work / "resources.tar") as archive:
            self.assertFalse(any("large.bin" in n for n in archive.getnames()))
        self.assertEqual(audit.command(["git", "status", "--porcelain"], work / "source"), "")

    def test_dependencies_keep_independent_checks_and_block_transitive_outputs(self):
        work = self.prepare()
        scenario = self.scenario()
        first = scenario["steps"][0]
        first.update(argv=[sys.executable, "-c", "raise SystemExit(1)"], repetitions=1, warmup=0, parser="exit")
        import copy
        independent = copy.deepcopy(first)
        independent.update(id="independent", argv=[sys.executable, "-c", "print('ok')"])
        blocked = copy.deepcopy(independent)
        blocked.update(id="blocked", depends_on=[first["id"]])
        downstream = copy.deepcopy(blocked)
        downstream.update(id="downstream", depends_on=["blocked"])
        scenario["steps"] += [independent, blocked, downstream]
        path = self.root / "dag.json"
        audit.write(path, scenario)
        self.assertEqual(audit.run_campaign(work, path, "dag"), 1)
        checks = audit.read(work / "runs/dag/results/checks.json")
        self.assertEqual([c["status"] for c in checks], ["failed", "passed", "skipped", "skipped"])
        for deps in (["unknown"], ["known-data"]):
            invalid = self.scenario()
            invalid["steps"][0]["depends_on"] = deps
            with self.assertRaises(ValueError):
                audit.validate_scenario(invalid)

    def test_benchmark_producer_array_contract(self):
        # emit_json in examples/bench.rs emits these four fields, nullable
        # numbers, and a pretty-printed top-level array, not JSONL.
        output = '[\n {"name":"Sine","x_real_time":2.0000,"peak_block":15.2500,"gated":true},\n {"name":"diagnostic","x_real_time":null,"peak_block":20.0000,"gated":false}\n]\n'
        rows = audit.parse_metrics("bench", output)
        self.assertEqual(rows[1]["unit"], "microseconds")
        self.assertEqual(rows[1]["value"], 15.25)
        self.assertEqual(rows[2]["direction"], "descriptive")
        for invalid in ('[]', '[null]', '[{"name":"bad","peak_block":NaN}]'):
            with self.assertRaises(ValueError):
                audit.parse_metrics("bench", invalid)

    def test_failed_suite_retains_counts_signal_and_last_test(self):
        work = self.prepare()
        scenario = self.scenario()
        scenario["steps"][0].update(argv=[sys.executable, "-c", "import os,signal; print('test failed_case ... FAILED', flush=True); print('test result: FAILED. 1310 passed; 1 failed', flush=True); os.kill(os.getpid(),signal.SIGTERM)"], parser="rust-tests", repetitions=1, warmup=0)
        path = self.root / "failure.json"
        audit.write(path, scenario)
        self.assertEqual(audit.run_campaign(work, path, "crash"), 1)
        run = work / "runs/crash"
        entry = audit.read(run / "manifest.json")["commands"][0]
        self.assertEqual(entry["signal"], "SIGTERM")
        self.assertIn("failed_case", entry["last_test"])
        rows = audit.read(run / "results/summary.json")
        self.assertEqual(next(r for r in rows if r["name"] == "tests_passed")["median"], 1310)

    def test_policy_profiles_and_workspace_targets(self):
        import controls
        for value in ({"jobs": 0}, {"jobs": -1}, {"cpus": "3-1"}, {"disk_floor_gib": float("nan")}):
            with self.assertRaises(ValueError):
                controls.validate_policy(value)
        with self.assertRaises(ValueError):
            controls.policy_environment({"disk_floor_gib": 1e30}, self.root)
        self.assertEqual(controls.policy_environment({}, self.root)["CARGO_INCREMENTAL"], "0")
        paths = {controls.target_for(self.root / "target", name) for name in (".", "clients/gui", "audit/fuzz", "clausters-editor/src-tauri")}
        self.assertEqual(len(paths), 4)
        with self.assertRaises(ValueError):
            controls.validate_build_contract({"argv": ["cargo", "test", "--release"], "profile": "debug"}, self.repo)
        with self.assertRaises(ValueError):
            controls.validate_build_contract({"argv": ["cargo", "test", "--features", "missing"]}, self.repo)

    def test_web_preflight_missing_assets_and_content_identity(self):
        import controls
        with self.assertRaisesRegex(ValueError, "Missing web vendor"):
            controls.web_preflight(self.repo)
        names = ("verovio/verovio.js", "verovio/verovio.wasm", "faust/libfaust-wasm.js", "faust/libfaust-wasm.wasm", "faust/libfaust-wasm.data", "faust/version")
        for name in names:
            p = self.repo / "clients/web/vendor" / name
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text("synthetic")
        for name in ("faust.pin", "verovio.pin"):
            p = self.repo / "third_party" / name
            p.parent.mkdir(exist_ok=True)
            p.write_text("synthetic pin")
        receipt = controls.web_preflight(self.repo)
        self.assertEqual(len(receipt["assets"]), 6)
        p.write_text("changed pin")
        self.assertNotEqual(receipt, controls.web_preflight(self.repo))

    def test_generated_diffs_survive_repeated_runs_without_checkout_drift(self):
        work = self.prepare()
        scenario = self.scenario()
        scenario["steps"][0].update(argv=[sys.executable, "-c", "from pathlib import Path; Path('tracked.txt').write_text('generated')"], parser="exit", repetitions=1, warmup=0, generated_outputs=["tracked.txt"])
        path = self.root / "generated.json"
        audit.write(path, scenario)
        for name in ("gen-one", "gen-two"):
            self.assertEqual(audit.run_campaign(work, path, name), 0)
            diff = work / "runs" / name / "evidence/known-data-0-generated.patch"
            self.assertIn("generated", diff.read_text())
            self.assertEqual(audit.command(["git", "status", "--porcelain"], work / "source"), "")

    def test_device_fps_missing_callbacks_and_safety_limits(self):
        import device
        def capture(cpu=30, frames=1800):
            def point(t, count):
                return {"status": {"t": t, "late_blocks": 0}, "server_timing": {"duration": {"count": 100 + count}, "over_deadline": 0}, "gui_timing": {"duration": {"count": count}}}
            return {"start": point(0, 0), "end": point(60, frames), "samples": [{"status": {"avg_cpu": cpu, "peak_cpu": 50}}]}
        self.assertEqual(device.assess_capture(capture(), True)["fps"], 30)
        for frames in (0, 3, 1000):
            with self.assertRaises(ValueError):
                device.assess_capture(capture(frames=frames), True)
        scenario = self.scenario()
        scenario["steps"][0].update(kind="sweep", levels=[0, 20], gui=True, screen_on=False,
                                     argv=["driver", "{level}", "{capture}"])
        with self.assertRaises(ValueError):
            audit.validate_scenario(scenario)
        scenario["steps"][0]["screen_on"] = True
        audit.validate_scenario(scenario)
        folder = self.root / "campaign"
        (folder / "evidence").mkdir(parents=True)
        (folder / "logs").mkdir()
        def driver(argv, cwd, env, timeout, stdout, stderr):
            stdout.write_text(""); stderr.write_text("")
            data = capture(cpu=20 if argv[1] == "0" else 50)
            audit.write(argv[2], {"capture": data})
            return {"status": "passed", "exit_code": 0, "elapsed_seconds": .1}
        result = device.execute_sweep(scenario["steps"][0], scenario["steps"][0]["argv"], self.repo, {}, folder, 0, driver)
        self.assertEqual(result["status"], "passed")
        self.assertEqual(audit.read(folder / "evidence/known-data-0/receipt.json")["representative_level"], 0)


    def test_cargo_router_routes_nested_workspace_invocations(self):
        work = self.prepare()
        fake = self.root / "fake-cargo"
        fake.write_text('#!/bin/sh\nprintf "%s" "$CARGO_TARGET_DIR"\n')
        fake.chmod(0o755)
        env = {**os.environ, "AUDIT_SOURCE": str(work / "source"), "AUDIT_TARGET": str(work / "target"), "AUDIT_REAL_CARGO": str(fake)}
        for cwd, expected in ((".", "target"), ("audit/fuzz", "target-fuzz")):
            result = audit.command([sys.executable, str(AUDIT / "scripts/cargo_router.py"), "--version"], work / "source" / cwd, env)
            self.assertEqual(result, str(work / expected))
        outside = self.root / "outside"
        outside.mkdir()
        result = audit.command([sys.executable, str(AUDIT / "scripts/cargo_router.py"), "--version"],
                               outside, {**env, "CARGO_TARGET_DIR": "/kept"})
        self.assertEqual(result, "/kept")

    def test_pytest_file_isolation_continues_after_native_signal(self):
        fake = self.root / "fake_pytest.py"
        fake.write_text("import sys,os,signal\nif '--collect-only' in sys.argv:\n print('test_a.py::one\\ntest_b.py::two')\nelif sys.argv[-1] == 'test_a.py':\n os.kill(os.getpid(),signal.SIGTERM)\nelse:\n print('test_b.py::two PASSED\\n1 passed')\n")
        result = subprocess.run([sys.executable, str(AUDIT / "scripts/isolate_pytest.py"), sys.executable, str(fake)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("SIGTERM", result.stdout)
        self.assertIn("test_b.py::two PASSED", result.stdout)

    def test_manual_gui_evidence_cannot_accept_blanked_screen(self):
        work = self.prepare()
        scenario = self.scenario(manual=True)
        scenario["steps"][-1]["id"] = "gui-device"
        path = self.root / "gui.json"
        audit.write(path, scenario)
        audit.run_campaign(work, path, "gui")
        artifact = self.root / "screen.txt"
        artifact.write_text("synthetic")
        payload = {"status": "passed", "operator": "test", "procedure": "test", "configuration": "test", "limitations": "synthetic", "reason": "test", "artifacts": [str(artifact)]}
        evidence = self.root / "gui-evidence.json"
        run = work / "runs/gui"
        for extra in ({}, {"screen_on": True}, {"screen_on": True, "gui_runs": [{"frames": 3, "elapsed_seconds": 60}]}):
            audit.write(evidence, {**payload, **extra})
            with self.assertRaises(ValueError):
                audit.attach(run, "gui-device", evidence)
        audit.write(evidence, {**payload, "screen_on": True, "gui_runs": [{"frames": 1800, "elapsed_seconds": 60}], "renderer": {"kind": "hardware", "adapter": "synthetic", "driver": "synthetic", "backend": "synthetic"}})
        audit.attach(run, "gui-device", evidence)
        audit.seal_record(run, "test")


    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler unavailable")
    def test_parser_against_executed_benchmark_json_emitter(self):
        source = (AUDIT.parent / "examples/bench.rs").read_text()
        # Compile the producer's actual record/emitter code without its DSP
        # workload, so a format change cannot leave a synthetic fixture green.
        fragment = source[source.index("struct Record {"):source.index("const SAMPLE_RATE:")]
        program = self.root / "producer.rs"
        program.write_text(fragment + '\nfn main() { record("actual", Some(2.0), Some(15.25), true); record("diagnostic", None, Some(20.0), false); emit_json(); }\n')
        binary = self.root / "producer"
        subprocess.run(["rustc", "--edition=2024", "-A", "dead_code", "-A", "unused_macros", str(program), "-o", str(binary)], check=True, capture_output=True)
        output = audit.command([str(binary), "--json"])
        rows = audit.parse_metrics("bench", output)
        self.assertEqual([r["value"] for r in rows], [2.0, 15.25, 20.0])
        self.assertEqual(rows[-1]["direction"], "descriptive")

    def test_optional_fuzz_tools_are_pinned_and_seeds_attested(self):
        import fuzz_tools
        work = self.prepare()
        binary = work / "tools/cargo/bin/cargo-fuzz"
        binary.parent.mkdir(parents=True)
        binary.write_text("synthetic tool bytes")
        real_command = audit.command
        def fake_command(argv, *args):
            if argv[0] == str(binary):
                return "cargo-fuzz 0.13.2"
            if argv[0] == "rustup":
                self.assertEqual(argv[2], "nightly-2026-10-15")
                return "rustc synthetic pinned nightly"
            return real_command(argv, *args)
        with patch("audit.command", side_effect=fake_command):
            path = fuzz_tools.attest(work, binary)
        receipt = audit.read(path)
        self.assertEqual(receipt["sha256"], audit.digest(binary))
        self.assertEqual(len(receipt["seeds"]), 4)
        self.assertEqual(receipt["cargo_fuzz"], "cargo-fuzz 0.13.2")


    def test_per_target_compiler_evidence_is_retained(self):
        work = self.prepare()
        scenario = self.scenario()
        artifact = {"reason": "compiler-artifact", "package_id": "synthetic#0.0.0", "manifest_path": "Cargo.toml", "target": {"name": "synthetic", "kind": ["lib"]}, "features": ["synth"], "profile": {"test": True}, "filenames": ["synthetic-test"], "fresh": False}
        text = json.dumps(artifact) + "\ntest result: ok. 2 passed; 0 failed"
        scenario["steps"][0].update(argv=[sys.executable, "-c", "print(" + repr(text) + ")"], parser="rust-tests", repetitions=1, warmup=0)
        path = self.root / "targets.json"
        audit.write(path, scenario)
        self.assertEqual(audit.run_campaign(work, path, "targets"), 0)
        entry = audit.read(work / "runs/targets/manifest.json")["commands"][0]
        self.assertEqual(entry["compiled_targets"][0]["features"], ["synth"])
        self.assertEqual(entry["compiled_targets"][0]["target"]["kind"], ["lib"])


if __name__ == "__main__":
    unittest.main()
