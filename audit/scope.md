# Scope and evaluation criteria

The audit covers every Rust crate in the project, with clausters-editor assigned to its own audit unit, including the server, GUI host, shared core, FFI, document, editing, applications, network, MIDI, notation, plugin hosting, browser bindings, fuzz tooling and test fixture crates. Client programs are audited independently, and the Rust packages they bind remain essential audited components. Assess native/browser and feature-specific paths separately and report unsupported configurations explicitly. prepare and run inventory tracked Cargo.toml package manifests dynamically; newly added crates cannot disappear from coverage just because a static list was not updated. The workspace scenario requires a per-manifest review matrix for components in addition to automatic tests. The editor crate is retained in the inventory and reviewed in the separate editor campaign.

| Area | Audio server | GUI host |
| --- | --- | --- |
| Correctness | DSP output, scheduling, node lifecycle, buses | Layout, gestures, drawing, server synchronization |
| Efficiency | Block/callback timing, throughput, memory | Frame timing, CPU/GPU work, transfers, memory |
| Robustness | Queue saturation, invalid inputs, overload recovery | Large inputs, event bursts, connection loss, recovery |
| Architecture | Bounded RT work, RT/non-RT separation | Shared behavior, compute/drawing/I/O boundaries |
| Maintainability | Contracts, duplication, useful tests, unsafe boundaries | Extension points, native/browser parity, useful tests |

Review code manually as well as using tools. A clean linter, coverage percentage or successful benchmark does not establish architectural quality. Record file/line references against the evaluated commit, evidence, severity and limitations for each finding.

ISO/IEC 25010:2023 and ISO/IEC 25023:2016 can organize quality criteria. They do not define this project's audio deadlines or GUI targets, and their editions require an explicit mapping if used formally. This scaffold makes no certification claim.

Acceptance criteria must be agreed and recorded before measurement. Draft scenarios deliberately contain no invented thresholds. Findings affecting the application should be transferred to the owning plan's Found by use or Future directions section when application changes are authorized; this directory does not replace those plans.

## Independent audit units

| Unit | Scenarios | Reference and acceptance boundary |
| --- | --- | --- |
| Components | workspace, audio, gui, combined | Every project component crate, including browser/FFI/plugin seams; device and interaction coverage retained separately |
| Python client | python | Reference API and behavior; package, native bindings, scheduling and examples |
| Web client | web | Same capabilities and calls as Python, with language idioms distinguished from gaps; own node/browser acceptance |
| Editor application | editor | Independent Tauri/Svelte/Python application and Rust workspace; own version, dependencies, UI and licensing |

Use separate preparation roots and records/baselines for each unit. A complete component scenario says nothing about client/editor quality; a complete client scenario says nothing about untested server/host paths. Python is the API/behavior reference, not a substitute for web measurements. New non-Rust packages require an explicit unit/scope review as well; package.json and pyproject.toml identities and lockfiles are retained with preparation. No package's results are averaged into another's score.
