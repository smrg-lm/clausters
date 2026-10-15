# Initial scenarios

All seven JSON specifications are ready to collect a first campaign. They combine executable automatic checks with explicit required manual checks. A ready scenario means its process is defined, not that any result exists. A campaign with missing manual evidence is partial and cannot be sealed.

- audio.json runs existing Synth-family correctness/RT/capacity tests, builds the existing DSP benchmark and collects three repetitions after one complete warmup. The example controls its own inner measurement duration. Separate device and code-review evidence is required, including coverage of the peer def family when claiming the complete server.
- gui.json runs the native host's default-feature library tests and builds the host, then requires real rendering/interaction and code review evidence. The suite's elapsed time is descriptive, never a rendering benchmark.
- combined.json requires three matched control/interference repetitions on a reserved machine. It is evidence-driven because the correct backend/device/input workload must be selected and recorded by the operator.

Build/test timeout is 1800 seconds; each DSP invocation has 1200 seconds. No compilation is inside the DSP sample timer. Logs include setup commands, warmup and measured repetitions. The runner does not suppress build failures, accept zero-test suites or reinterpret unsupported measurements as zero.

Initial performance acceptance is descriptive until an accepted comparable baseline exists. Automatic correctness requires exit zero and parser assertions. Manual procedures define correctness and evidence requirements without claiming universal CPU/GPU thresholds. Narrower feature coverage must remain visible in the report. See ../formats.md for the command schema and ../procedures/ for device/review protocols.


workspace.json is required for complete component coverage, using a dynamic crate inventory and per-manifest manual matrix; audio/gui/combined cannot replace it. python.json, web.json and editor.json are separate units with separate preparation roots. Python is the reference client; web requires side-by-side API/example parity review. Their offline dependency setup is in ../tools/package-setup.md. Full project coverage means reviewing each unit's records, not accepting one scenario as the entire project.
