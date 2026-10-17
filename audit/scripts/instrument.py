#!/usr/bin/env python3
"""Export Linux callback/frame instrumentation from a clean disposable clone."""
import argparse
from pathlib import Path
import subprocess
import sys
from audit import AUDIT, command, export_patch, replay


def build_patch(source, output):
    if not sys.platform.startswith("linux"):
        raise ValueError("Device instrumentation supports Linux native builds only")
    source, output = Path(source).resolve(), Path(output).resolve()
    if command(["git", "status", "--porcelain"], source):
        raise ValueError("Instrumentation needs a clean disposable clone")
    base = command(["git", "rev-parse", "HEAD"], source)
    changes = {
        "src/server/mod.rs": [("pub mod backend;", "pub mod backend;\n#[cfg(target_os = \"linux\")]\npub mod audit_timing;")],
        "src/server/backend.rs": [
            ("                    stream.play()?;", '                    crate::server::audit_timing::TIMING.set_rate(rate);\n                    crate::server::audit_timing::start_writer("CLAUSTERS_AUDIT_TIMING");\n                    crate::server::audit_timing::start_slow_writer();\n                    stream.play()?;'),
            ("    device.build_output_stream(", "    let epoch = crate::server::audit_timing::epoch();\n    device.build_output_stream("),
            ("        move |data: &mut [T], _| {", "        move |data: &mut [T], _| {\n            let callback_start = std::time::Instant::now();\n            let audit_before = crate::server::audit_timing::before(callback_start);"),
            ("                *s = T::from_sample(adapter.next_sample());\n            }", "                *s = T::from_sample(adapter.next_sample());\n            }\n            crate::server::audit_timing::TIMING.record(epoch, callback_start, frames as u64);\n            crate::server::audit_timing::after(epoch, &audit_before, frames as u64);")],
        "src/server/engine.rs": [
            ("        let elapsed_nanos = meter_start.elapsed_nanos();", "        let elapsed_nanos = meter_start.elapsed_nanos();\n        crate::server::audit_timing::BLOCKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n        crate::server::audit_timing::BLOCK_MAX_NS.fetch_max(elapsed_nanos, std::sync::atomic::Ordering::Relaxed);"),
            ("    fn apply(&mut self, cmd: Cmd) {", "    fn apply(&mut self, cmd: Cmd) {\n        crate::server::audit_timing::APPLIED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);")],
        "clients/gui/src/host/gui/mod.rs": [("mod app;", "mod app;\nmod audit_timing;")],
        "clients/gui/src/gpu.rs": [
            ("        let mut gpu = Self {\n            shown: Some((window, surface)),", '''        if let Ok(path) = std::env::var("CLAUSTERS_AUDIT_RENDERER") {
            let info = adapter.get_info();
            let value = serde_json::json!({
                "adapter": info.name, "driver": info.driver,
                "driver_info": info.driver_info,
                "backend": format!("{:?}", info.backend),
                "kind": if info.device_type == wgpu::DeviceType::Cpu { "software" } else { "hardware" },
                "presentation": "unsupported: no presentation timestamp collector",
                "present_mode": format!("{:?}", config.present_mode),
                "width": config.width, "height": config.height
            });
            if let Err(error) = std::fs::write(path, value.to_string()) {
                eprintln!("audit renderer capture failed: {error}");
            }
        }
        let mut gpu = Self {
            shown: Some((window, surface)),''')],
        "clients/gui/src/host/gui/app.rs": [
            ("    fn resumed(&mut self, event_loop: &ActiveEventLoop) {", '    fn resumed(&mut self, event_loop: &ActiveEventLoop) {\n        super::audit_timing::start_writer("CLAUSTERS_AUDIT_GUI_TIMING");'),
            ("    fn draw(&mut self, def_id: i32, capture: bool) -> Option<Result<frame::Readback, String>> {", '    fn draw(&mut self, def_id: i32, capture: bool) -> Option<Result<frame::Readback, String>> {\n        let audit_epoch = super::audit_timing::epoch();\n        let audit_start = std::time::Instant::now();'),
            ("        if let Some(started) = timed {", "        if matches!(drawn, frame::Drawn::Whole | frame::Drawn::Live) {\n            super::audit_timing::TIMING.record(audit_epoch, audit_start, 0);\n        }\n        if let Some(started) = timed {")]
    }
    # Validate every anchor before writing anything; source drift must be explicit.
    rendered = {}
    for name, replacements in changes.items():
        text = (source / name).read_text()
        for before, after in replacements:
            if text.count(before) != 1:
                raise ValueError(f"Instrumentation anchor moved: {name}: {before!r}")
            text = text.replace(before, after)
        rendered[name] = text
    template = (AUDIT / "instrumentation/timing.rs.in").read_text()
    rendered["src/server/audit_timing.rs"] = template
    rendered["clients/gui/src/host/gui/audit_timing.rs"] = '#![allow(dead_code)] // The GUI uses only the shared histogram recorder.\n' + template
    for name, text in rendered.items():
        (source / name).write_text(text)
    subprocess.run(["rustfmt", "--edition", "2024", *rendered], cwd=source, check=True)
    export_patch(source, base, output)
    return replay(source, base, output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    print(build_patch(args.source, args.output))
