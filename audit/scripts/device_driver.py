"""Device audit driver, run through a prepared snapshot.

One run = one fresh server (and GUI when asked), a fixed load, a warm-up,
then a held capture. Writes one JSON record with the instrumented timing
deltas, the server's own counters, PipeWire's per-node error counts and
per-process CPU/RSS samples. Nothing here is a pass/fail judgement.
"""
import argparse, json, math, os, socket, subprocess, sys, threading, time
from pathlib import Path

from device import assess_capture, wait_for_freed
from telemetry import gpu_sample, gpu_delta, host_sample, pipewire_nodes, xrun_delta

CLK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")


def load_def():
    from clausters.defs import SynthDef, control, out, sine
    # Ten sines summed and scaled by a zero amp: the DSP runs, nothing sounds.
    amp = control("amp", 0.0)
    f = control("freq", 440.0)
    s = sine(f)
    for i in range(2, 11):
        s = s + sine(f * (1.0 + 0.01 * i))
    return SynthDef("audit_load", out(0.0, s * amp))


def source_def():
    from clausters.defs import SynthDef, control, out, sine
    freq = control("freq", 880.0, lag=0.05)
    amp = control("amp", 0.02)
    left = sine(freq)
    side = sine(freq * 1.5)
    right = left * control("wm", 1.0, lag=0.05) + side * control("ws", 0.0, lag=0.05)
    return SynthDef("audit_source", out(0.0, left * amp), out(1.0, right * amp))


def pids_of(exe):
    found = []
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                if os.readlink(f"/proc/{p}/exe") == exe:
                    found.append(int(p))
            except OSError:
                pass
    return found


def proc_sample(pid):
    try:
        st = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        utime, stime = int(st[11]), int(st[12])
        rss = int(st[21]) * PAGE
        return {"t": time.monotonic(), "cpu_s": (utime + stime) / CLK, "rss": rss}
    except OSError:
        return None


def pw_top():
    try:
        command = subprocess.run(["pw-top", "-b", "-n", "2"], capture_output=True,
                                 text=True, timeout=10)
        txt = command.stdout
    except Exception as e:
        return {"error": str(e)}
    # Keep the last iteration only: each one starts with the header line.
    last = txt.split("S   ID")[-1].splitlines()[1:]
    rows = []
    for line in last:
        parts = line.split()
        if len(parts) >= 9 and parts[1].isdigit():
            rows.append({"state": parts[0], "id": int(parts[1]), "quant": parts[2],
                         "err": int(parts[8]) if parts[8].isdigit() else None,
                         "name": parts[-1], "line": line})
    return {"rows": rows, "stdout": txt, "stderr": command.stderr,
            "exit_code": command.returncode,
            "status": "available" if command.returncode == 0 and rows else "unsupported"}


def rt_state(pid):
    out = {}
    try:
        lim = subprocess.run(["prlimit", "--pid", str(pid), "--rttime", "--noheadings", "-o", "SOFT,HARD"],
                             capture_output=True, text=True).stdout.split()
        out["rttime"] = lim
        for t in os.listdir(f"/proc/{pid}/task"):
            comm = open(f"/proc/{pid}/task/{t}/comm").read().strip()
            if comm.startswith("pw_") or "audio" in comm or "cpal" in comm:
                out.setdefault("threads", {})[comm] = os.sched_getscheduler(int(t)) & ~0x40000000
    except OSError as e:
        out["error"] = str(e)
    return out


def read_json(path):
    for _ in range(20):
        try:
            return json.loads(Path(path).read_text())
        except (OSError, ValueError):
            time.sleep(0.05)
    return None


def status(server):
    st = server.status()
    return {"t": time.monotonic(), "avg_cpu": st.avg_cpu, "peak_cpu": st.peak_cpu,
            "late_blocks": st.late_blocks, "ugens": getattr(st, "ugens", None),
            "synths": getattr(st, "synths", None), "actual_sample_rate": st.actual_sample_rate,
            "nominal_sample_rate": st.nominal_sample_rate}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--capture", required=True)
    ap.add_argument("--server", required=True)
    ap.add_argument("--gui-bin")
    ap.add_argument("--ffi-lib", help="Exact shared-core library; defaults beside the server binary")
    ap.add_argument("--nodes", type=int, default=0)
    ap.add_argument("--source", action="store_true")
    ap.add_argument("--gui", action="store_true")
    ap.add_argument("--screen-on", action="store_true")
    ap.add_argument("--min-fps", type=float, default=20)
    ap.add_argument("--warmup", type=float, default=10)
    ap.add_argument("--seconds", type=float, default=60)
    ap.add_argument("--slow-us", type=int)
    ap.add_argument("--uninstrumented", action="store_true",
                    help="Control run: record unavailable timing explicitly")
    a = ap.parse_args()
    if a.nodes < 0 or not all(math.isfinite(v) and v > 0 for v in (a.warmup, a.seconds, a.min_fps)):
        ap.error("nodes must be nonnegative and durations/fps positive and finite")
    if a.gui and (not a.screen_on or not a.gui_bin):
        ap.error("GUI capture requires --screen-on and --gui-bin; keep display awake")
    if a.slow_us is not None and a.slow_us <= 0:
        ap.error("slow threshold must be positive")
    if a.slow_us and a.uninstrumented:
        ap.error("slow capture is not an uninstrumented control")
    path = Path(a.capture).resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        ap.error("capture must be new")
    os.environ["CLAUSTERS_BIN"] = str(Path(a.server).resolve(strict=True))
    ffi = Path(a.ffi_lib) if a.ffi_lib else Path(a.server).resolve().parent / "libclausters_ffi.so"
    os.environ["CLAUSTERS_FFI_LIB"] = str(ffi.resolve(strict=True))
    if a.gui_bin:
        os.environ["CLAUSTERS_GUI_BIN"] = str(Path(a.gui_bin).resolve(strict=True))
    stimer, gtimer = str(path) + ".audio.json", str(path) + ".gui.json"
    for key in ("CLAUSTERS_AUDIT_TIMING", "CLAUSTERS_AUDIT_GUI_TIMING",
                "CLAUSTERS_AUDIT_SLOW_US", "CLAUSTERS_AUDIT_SLOW"):
        os.environ.pop(key, None)
    if not a.uninstrumented:
        os.environ["CLAUSTERS_AUDIT_TIMING"] = stimer
        os.environ["CLAUSTERS_AUDIT_GUI_TIMING"] = gtimer
    os.environ["CLAUSTERS_AUDIT_RENDERER"] = str(path) + ".renderer.json"
    if a.slow_us:
        os.environ["CLAUSTERS_AUDIT_SLOW_US"] = str(a.slow_us)
        os.environ["CLAUSTERS_AUDIT_SLOW"] = str(path) + ".slow.json"
    rec = {"args": vars(a), "env": {k: os.environ.get(k) for k in (
        "PIPEWIRE_QUANTUM", "CLAUSTERS_BIN", "CLAUSTERS_GUI_BIN", "CLAUSTERS_FFI_LIB")},
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    session, th = None, None
    stop = threading.Event()
    result = 1
    try:
        from clausters import Session
        from clausters.defs import Synth
        from clausters.gui import panel, phasescope, signal, spectrum, view
        # Reserve distinct loopback port numbers, avoiding the ordinary session.
        # The device procedure reserves the machine; a bind race is still a
        # failed prerequisite rather than permission to use an existing server.
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as tcp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
            tcp.bind(("127.0.0.1", 0))
            audio_port = tcp.getsockname()[1]
            udp.bind(("127.0.0.1", audio_port))
        rec["audio_port"] = audio_port
        session = Session.live(port=audio_port, workers=0, data_dir=str(path) + ".server-data")
        server = session.server
        rec["server_pids"] = pids_of(os.path.realpath(a.server))
        if len(rec["server_pids"]) != 1:
            raise ValueError("Device trial requires exactly one isolated server binary process")
        load_def().send(server)
        nodes = [Synth("audit_load", {"freq": 440.0 + i % 50}, server=server) for i in range(a.nodes)]
        if a.source or a.gui:
            source_def().send(server)
            src = Synth("audit_source", {}, server=server)
            def sweep_source():
                t0 = time.monotonic()
                while not stop.is_set():
                    t = time.monotonic() - t0
                    angle = math.pi * (0.5 - 0.5 * math.cos(2 * math.pi * t / 6))
                    src.set({"wm": math.cos(angle), "ws": math.sin(angle),
                             "freq": 880 * 2 ** (0.5 * math.sin(2 * math.pi * t / 11))})
                    stop.wait(.03)
            th = threading.Thread(target=sweep_source, daemon=True)
            th.start()
        if a.gui:
            with socket.socket() as tcp:
                tcp.bind(("127.0.0.1", 0))
                gui_port = tcp.getsockname()[1]
            rec["gui_port"] = gui_port
            session.gui(port=gui_port, data_dir=str(path) + ".gui-data")
            window = view(
                panel(phasescope(0, name="gonio", window_ms=30.0, label="goniometer"),
                      spectrum(0, name="spectrum", fft_size=2048, log_freq=True,
                               peak_hold=True, navigable=True, label="spectrum"), layout="row", h=200),
                signal(view="spectrogram", bus=0, retention=8.0, navigable=True,
                       name="waterfall", window_size=1024, freq_scale="log", label="waterfall"),
                title="audit: analyzer", w=760, h=640, layout="col").open()
            rec["gui_pids"] = pids_of(os.path.realpath(a.gui_bin))
            if len(rec["gui_pids"]) != 1:
                raise ValueError("Device trial requires exactly one isolated GUI binary process")
        pids = rec["server_pids"] + rec.get("gui_pids", [])
        def snapshot():
            return {"rt": [rt_state(p) for p in rec["server_pids"]],
                    "server_timing": read_json(stimer) if not a.uninstrumented else None,
                    "gui_timing": read_json(gtimer) if a.gui and not a.uninstrumented else None,
                    "status": status(server), "pw": pw_top(),
                    "proc": {p: proc_sample(p) for p in pids},
                    "gpu": gpu_sample(rec.get("gui_pids", [])), "host": host_sample()}
        time.sleep(a.warmup)
        server.status()
        rec["pipewire_identity"] = pipewire_nodes(rec["server_pids"])
        capture = {"start": snapshot(), "samples": []}
        rec["capture"] = capture
        until = time.monotonic() + a.seconds
        while time.monotonic() < until:
            time.sleep(min(1, max(0, until - time.monotonic())))
            capture["samples"].append({"status": status(server), "proc": {p: proc_sample(p) for p in pids},
                                       "gpu": gpu_sample(rec.get("gui_pids", []))})
        capture["end"] = snapshot()
        if a.uninstrumented:
            rec["assessment"] = {"timing": "unsupported: uninstrumented control",
                                 "mean_cpu": sum(s["status"]["avg_cpu"] for s in capture["samples"]) / len(capture["samples"]),
                                 "late": capture["end"]["status"]["late_blocks"] - capture["start"]["status"]["late_blocks"]}
        else:
            rec["assessment"] = assess_capture(capture, a.gui, a.min_fps)
        rec["gpu_assessment"] = gpu_delta([capture["start"]["gpu"],
            *[s["gpu"] for s in capture["samples"]], capture["end"]["gpu"]])
        rec["xrun_assessment"] = xrun_delta(rec["pipewire_identity"], capture["start"]["pw"], capture["end"]["pw"])
        if a.gui:
            # Readback adds work: do it after the timed interval.
            rec["renderer"] = ({"status": "unsupported", "reason": "Uninstrumented binary has no adapter identity writer"}
                               if a.uninstrumented else read_json(os.environ["CLAUSTERS_AUDIT_RENDERER"]))
            if not a.uninstrumented and not rec["renderer"]:
                raise ValueError("Missing instrumented renderer identity")
            rec["screenshot"] = session.gui_host.capture(window, str(path) + ".png")
        stop.set()
        if th:
            th.join(timeout=5)
        for node in nodes:
            node.free()
        owned = [node.id for node in nodes]
        if a.source or a.gui:
            src.free()
            owned.append(src.id)
        server.sync()
        rec["node_cleanup"] = wait_for_freed(server, owned)
        rec["after_free"] = status(server)
        if a.gui and window.closed:
            raise ValueError("GUI window closed during capture")
        result = 0
    except Exception as exc:
        rec["error"] = repr(exc)
    finally:
        stop.set()
        if th:
            th.join(timeout=5)
        if session:
            try:
                session.close()
            except Exception as exc:
                rec["cleanup_error"] = repr(exc)
                result = 1
        path.write_text(json.dumps(rec, allow_nan=False) + "\n")
    return result


if __name__ == "__main__":
    sys.exit(main())
