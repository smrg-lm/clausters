"""Compile and exercise the measurement template outside every app workspace."""
from pathlib import Path
import json
import shutil
import subprocess
import tempfile
import unittest
import sys

AUDIT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AUDIT / "scripts"))
from paired import histogram_delta

RUST_TESTS = r'''
#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::AtomicUsize;
    thread_local! { static TRACK: Cell<bool> = const { Cell::new(false) }; }
    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    struct Count;
    // SAFETY: forwards every allocation/deallocation unchanged to System.
    unsafe impl GlobalAlloc for Count {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACK.try_with(Cell::get).unwrap_or(false) {
                ALLOCATIONS.fetch_add(1, Relaxed);
            }
            // SAFETY: the caller supplies the GlobalAlloc layout contract.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: forwards the caller's original pointer and layout.
            unsafe { System.dealloc(ptr, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Count = Count;

    #[test]
    fn serializes_sparse_histograms_for_the_python_reader() {
        let hist = Hist::new();
        hist.add(BUCKET_NS * BUCKETS as u64 + 7);
        let before = hist.json();
        hist.add(0);
        hist.add(BUCKET_NS);
        let path = std::env::var("AUDIT_HISTOGRAM_CONTRACT").unwrap();
        std::fs::write(path, format!("[{},{}]", before, hist.json())).unwrap();
    }

    #[test]
    fn histogram_boundaries_and_overflow_keep_true_maximum() {
        let hist = Hist::new();
        hist.add(0);
        hist.add(BUCKET_NS);
        hist.add(BUCKET_NS * BUCKETS as u64 + 7);
        assert_eq!(hist.buckets[0].load(Relaxed), 1);
        assert_eq!(hist.buckets[1].load(Relaxed), 1);
        assert_eq!(hist.buckets[BUCKETS - 1].load(Relaxed), 1);
        assert_eq!(hist.count.load(Relaxed), 3);
        assert_eq!(hist.max_ns.load(Relaxed), BUCKET_NS * BUCKETS as u64 + 7);
    }

    #[test]
    fn recording_allocates_nothing_and_publishes_bounded_slow_events() {
        let timing = Timing::new();
        timing.set_rate(48000);
        let epoch = Instant::now();
        SLOW_NS.store(0, Relaxed);
        TRACK.with(|v| v.set(true));
        let b = before(Instant::now());
        timing.record(epoch, b.start, 128);
        after(epoch, &b, 128);
        TRACK.with(|v| v.set(false));
        assert_eq!(ALLOCATIONS.load(Relaxed), 0);
        assert_eq!(READY[0].load(Acquire), 1);
        assert_eq!(SLOW[0][2].load(Relaxed), 128);
        for _ in 0..SLOW_SLOTS + 1 {
            let b = before(Instant::now());
            after(epoch, &b, 128);
        }
        assert!(SLOW_N.load(Relaxed) > SLOW_SLOTS as u64);
        assert_eq!(READY.iter().filter(|v| v.load(Acquire) == 1).count(), SLOW_SLOTS);
        assert_eq!(timing.duration.count.load(Relaxed), 1);
        // JSON formatting runs off the RT thread, after publication.
        assert!(slow_json().contains("cpu_start"));
    }
}
'''


@unittest.skipUnless(shutil.which("cargo"), "Rust toolchain unavailable")
class TimingTests(unittest.TestCase):
    def test_template_compiles_lints_and_records_without_allocation(self):
        import os
        if not __import__("sys").platform.startswith("linux"):
            self.skipTest("Linux-only instrumentation")
        with tempfile.TemporaryDirectory(prefix="audit-timing-test-") as folder:
            root = Path(folder)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="audit-timing-contract"\nversion="0.0.0"\nedition="2024"\n[dependencies]\nlibc="0.2"\n')
            (root / "src/lib.rs").write_text((AUDIT / "instrumentation/timing.rs.in").read_text() + RUST_TESTS)
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target"), "CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0"}
            env["AUDIT_HISTOGRAM_CONTRACT"] = str(root / "histograms.json")
            # The lockfile resolves from the local registry cache when it has
            # libc, and from the network only where it does not (a fresh CI
            # runner); everything after runs offline against that lockfile.
            locked = subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root, env=env,
                                    capture_output=True, text=True, timeout=120)
            if locked.returncode:
                fetched = subprocess.run(["cargo", "fetch"], cwd=root, env=env,
                                         capture_output=True, text=True, timeout=300)
                self.assertEqual(fetched.returncode, 0, fetched.stdout + fetched.stderr)
            for argv in (["cargo", "test", "--offline"], ["cargo", "clippy", "--offline", "--all-targets", "--", "-D", "warnings"]):
                result = subprocess.run(argv, cwd=root, env=env, capture_output=True, text=True, timeout=120)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            before, after = json.loads((root / "histograms.json").read_text())
            self.assertEqual(histogram_delta(before, after), {"mean_us": 5, "p99_upper_us": 20})
