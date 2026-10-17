"""Contracts for optional GPU evidence, paired controls and noise calibration."""
import copy
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import baseline
import device
import paired
from paired import CONDITIONS, histogram_delta, order
from telemetry import gpu_delta, gpu_sample, parse_fdinfo, pipewire_nodes, xrun_delta


class DeviceCampaignTests(unittest.TestCase):
    def test_cleanup_waits_for_owned_nodes_and_allows_foreign_nodes(self):
        snapshots = iter(([1, 2, 99], [2, 99], [99]))
        server = SimpleNamespace(query_tree=lambda **kw: SimpleNamespace(
            walk=lambda: [SimpleNamespace(id=n) for n in next(snapshots)]))
        with patch.object(device.time, "sleep"):
            result = device.wait_for_freed(server, [1, 2])
        self.assertEqual(result["remaining_node_ids"], [])
        self.assertEqual(result["observed_node_ids"], [99])

    def test_cleanup_fails_if_an_owned_node_does_not_leave_the_tree(self):
        server = SimpleNamespace(query_tree=lambda **kw: SimpleNamespace(
            walk=lambda: [SimpleNamespace(id=1)]))
        with patch.object(device.time, "monotonic", side_effect=[0, 0, 1]):
            with self.assertRaisesRegex(ValueError, "Trial nodes remain"):
                device.wait_for_freed(server, [1], timeout=.1)

    def test_pipewire_process_identity_can_belong_to_the_parent_client(self):
        objects = [
            {"id": 10, "type": "PipeWire:Interface:Client", "info": {"props": {"application.process.id": 123}}},
            {"id": 11, "type": "PipeWire:Interface:Client", "info": {"props": {"application.process.id": 456}}},
            {"id": 20, "type": "PipeWire:Interface:Node", "info": {"props": {"client.id": 10}}},
            {"id": 21, "type": "PipeWire:Interface:Node", "info": {"props": {"application.process.id": "123"}}},
            {"id": 22, "type": "PipeWire:Interface:Node", "info": {"props": {"client.id": 11}}},
            {"id": 40, "type": "PipeWire:Interface:Link", "info": {"output-node-id": 20, "input-node-id": 30}},
        ]
        output = SimpleNamespace(returncode=0, stdout=json.dumps(objects), stderr="")
        with patch("telemetry.subprocess.run", return_value=output):
            result = pipewire_nodes([123])
        self.assertEqual(result["stream_nodes"], [20, 21])
        self.assertEqual(result["sink_nodes"], [30])
        self.assertEqual(result["status"], "available")

    def test_xruns_are_scoped_to_stream_and_sink_and_missing_is_not_zero(self):
        identity = {"status": "available", "stream_nodes": [1], "sink_nodes": [2]}
        first = {"status": "available", "rows": [{"id": 1, "err": 3}, {"id": 2, "err": 0}, {"id": 9, "err": 0}]}
        last = {"status": "available", "rows": [{"id": 1, "err": 3}, {"id": 2, "err": 1}, {"id": 9, "err": 10}]}
        self.assertEqual(xrun_delta(identity, first, last)["nodes"], {"1": 0, "2": 1})
        last["rows"].pop(1)
        self.assertEqual(xrun_delta(identity, first, last)["status"], "unsupported")
        last["rows"] = [{"id": 1, "err": 0}, {"id": 2, "err": 0}]
        self.assertEqual(xrun_delta(identity, first, last)["status"], "unsupported")

    def test_gpu_deduplicates_clients_and_retains_raw_evidence(self):
        with tempfile.TemporaryDirectory() as root:
            proc = Path(root)
            folder = proc / "10/fdinfo"
            folder.mkdir(parents=True)
            text = "drm-driver: i915\ndrm-pdev: 0000:00:02.0\ndrm-client-id: 7\ndrm-engine-render: 100 ns\n"
            (folder / "3").write_text(text)
            (folder / "4").write_text(text)
            sample = gpu_sample([10], proc)
            self.assertEqual(sample["status"], "available")
            self.assertEqual(len(sample["clients"]), 1)
            self.assertEqual(len(sample["raw_fdinfo"]), 2)
            self.assertEqual(sample["presentation"]["status"], "unsupported")
            self.assertIsNone(parse_fdinfo("pos: 0\n"))
            self.assertEqual(gpu_sample([20], proc)["status"], "unsupported")
            with self.assertRaises(ValueError):
                parse_fdinfo(text.replace("100 ns", "100 ticks"))

    def test_gpu_decreasing_counters_wait_for_catchup_and_changed_coverage_is_unsupported(self):
        def sample(t, value):
            return {"t": t, "status": "available", "clients": {"i915:pci:7": {"engines_ns": {"render": value}}}}
        samples = [sample(1, 100), sample(2, 90), sample(3, 1000000100)]
        self.assertAlmostEqual(gpu_delta(samples)["engines"][0]["busy_percent"], 50)
        self.assertEqual(gpu_delta([sample(1, 100), sample(2, 90)])["status"], "unsupported")
        samples[-1]["clients"] = {}
        self.assertEqual(gpu_delta(samples)["status"], "unsupported")

    def test_pair_order_covers_all_controls_and_rotates(self):
        self.assertEqual(len(CONDITIONS), 6)
        for index in range(3):
            self.assertEqual(set(order(index)), set(CONDITIONS))
        self.assertNotEqual(order(0), order(1))
        self.assertNotEqual(order(0)[0], order(2)[0])

    def test_paired_collection_retains_all_conditions_and_rejects_changed_load(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "binary"
            binary.write_bytes(b"synthetic identity, never executed")
            args = SimpleNamespace(output=str(root / "captures"), nodes=20, repetitions=3,
                seconds=60, warmup=10, server=str(binary), gui_bin=str(binary),
                control_server=str(binary), control_gui=str(binary), ffi_lib=str(binary))
            def execute(argv, cwd, env, timeout, stdout, stderr):
                capture = Path(argv[argv.index("--capture") + 1])
                instrumented = "--uninstrumented" not in argv
                hist0 = {"bucket_ns": 10000, "count": 100, "sum_ns": 1000000, "buckets": [100, 0]}
                hist1 = {"bucket_ns": 10000, "count": 110, "sum_ns": 1100000, "buckets": [110, 0]}
                def point(hist):
                    return {"status": {"actual_sample_rate": 48000}, "proc": {},
                            "server_timing": {"duration": hist, "frames_min": 128, "frames_max": 128} if instrumented else None,
                            "gui_timing": None}
                record = {"args": {"nodes": int(argv[argv.index("--nodes") + 1]), "seconds": 60, "warmup": 10},
                          "env": {"PIPEWIRE_QUANTUM": "128/48000"},
                          "assessment": {"mean_cpu": 25, "late": 0},
                          "capture": {"start": point(hist0), "end": point(hist1)},
                          "gpu_assessment": {"status": "unsupported"}}
                capture.write_text(json.dumps(record))
                return {"status": "passed", "exit_code": 0}
            with patch.object(paired, "execute", side_effect=execute), redirect_stdout(io.StringIO()) as output:
                paired.collect(args)
            receipt = json.loads((root / "captures/receipt.json").read_text())
            self.assertEqual(len(receipt["trials"]), 18)
            self.assertTrue(output.getvalue().strip())
            profile = paired.compatibility(root / "captures")
            self.assertEqual(len(profile["conditions"]), 6)
            trial = receipt["trials"][-1]
            path = root / "captures" / trial["capture"]
            record = json.loads(path.read_text())
            record["args"]["nodes"] += 1
            path.write_text(json.dumps(record))
            trial["sha256"] = paired.digest(path)
            (root / "captures/receipt.json").write_text(json.dumps(receipt))
            with self.assertRaises(ValueError):
                paired.compatibility(root / "captures")

    def test_histogram_excludes_warmup_and_refuses_overflow(self):
        first = {"bucket_ns": 10000, "count": 100, "sum_ns": 9000000, "buckets": [0, 100, 0]}
        last = {"bucket_ns": 10000, "count": 110, "sum_ns": 9100000, "buckets": [10, 100, 0]}
        self.assertEqual(histogram_delta(first, last), {"mean_us": 10, "p99_upper_us": 10})
        last["buckets"] = [0, 100, 10]
        with self.assertRaises(ValueError):
            histogram_delta(first, last)

    def test_baseline_requires_same_code_patch_and_repeated_nonzero_noise(self):
        manifest = {"source_commit": "same", "instrumentation": {"patch_sha256": "patch"}}
        summary = [{"step": "trial", "name": "cpu", "unit": "percent", "direction": "lower",
                    "count": 3, "min": 10, "max": 12}]
        comparison = {"results": [{"step": "trial", "metric": "cpu", "unit": "percent", "direction": "lower",
                                    "base_median": 10, "candidate_median": 11}]}
        def read(path):
            return copy.deepcopy(manifest if path.name == "manifest.json" else summary)
        with patch.object(baseline, "verify_record"), patch.object(baseline, "read", side_effect=read), \
                patch.object(baseline, "compare", return_value=comparison), patch.object(baseline, "digest", return_value="hash"):
            result = baseline.establish(["one", "two"], "reviewer", "reason")
            self.assertAlmostEqual(result["tolerance"], .25)
            summary[0]["min"] = 0
            with self.assertRaises(ValueError):
                baseline.establish(["one", "two"], "reviewer", "reason")
            summary[0]["min"] = 10
            summary[0]["count"] = 2
            with self.assertRaises(ValueError):
                baseline.establish(["one", "two"], "reviewer", "reason")
        with self.assertRaises(ValueError):
            baseline.establish(["one", "one"], "reviewer", "reason")
