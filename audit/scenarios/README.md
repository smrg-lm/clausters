# Initial scenarios

The JSON specifications are ready to collect a first campaign. They combine executable automatic checks with explicit required manual checks. A ready scenario means its process is defined, not that any result exists. A campaign with missing manual evidence is partial and cannot be sealed.

- audio.json runs existing Synth-family correctness/RT/capacity tests, builds the existing DSP benchmark and collects three repetitions after one complete warmup. The example controls its own inner measurement duration. Separate device and code-review evidence is required, including coverage of the peer def family when claiming the complete server.
- gui.json runs the native host's default-feature library tests and builds the host, then requires real rendering/interaction and code review evidence. The suite's elapsed time is descriptive, never a rendering benchmark.
- combined.json requires three matched control/interference repetitions on a reserved machine. It is evidence-driven because the correct backend/device/input workload must be selected and recorded by the operator.

Build/test timeout is 1800 seconds; each DSP invocation has 1200 seconds. No compilation is inside the DSP sample timer. Logs include setup commands, warmup and measured repetitions. The runner does not suppress build failures, accept zero-test suites or reinterpret unsupported measurements as zero.

Initial performance acceptance is descriptive until an accepted comparable baseline exists. Automatic correctness requires exit zero and parser assertions. Manual procedures define correctness and evidence requirements without claiming universal CPU/GPU thresholds. Narrower feature coverage must remain visible in the report. See ../formats.md for the command schema and ../procedures/ for device/review protocols.


workspace.json is required for complete component coverage, using a dynamic crate inventory and per-manifest manual matrix; audio/gui/combined cannot replace it. python.json, web.json and editor.json are separate units with separate preparation roots. Python is the reference client; web requires side-by-side API/example parity review. Their offline dependency setup is in ../tools/package-setup.md. Full project coverage means reviewing each unit's records, not accepting one scenario as the entire project.


Correctness uses the debug profile; release is reserved for benchmark/device workloads. The native workspace selection excludes `clausters-web` and `clausters-nrt-web`, whose shared server feature unification otherwise changes native no-default builds. The workspace scenario runs `scripts/check-wasm.sh` separately; the crate inventory and per-manifest review still include every crate. Profile/feature declarations are checked against Cargo arguments and selected manifests.

`depends_on` names earlier step IDs. Only declared dependents are skipped after a failure; independent checks continue. Manual evidence cannot make a failed automatic prerequisite disappear. `web-tests` depends on both native builds and the web build; DSP collection depends on its benchmark build. Empty suites and failed warmups still stop repetitions of their own step.

`device.json` builds and runs the Linux instrumented sweep, with required manual paired-control review afterwards. Use a replay-verified patch, a reserved machine and an awake screen. `fuzz.json` generates an offline lock, retains it as evidence and runs stable lint in an independent target; optional sanitizer coverage remains a separately reviewed device-independent activity.
