# Source and implementation review

Review the fixed source commit, not the current main checkout. Retain a coverage table naming modules, reviewer, file/line references, inspected contracts and test evidence. Record severity, impact, confidence, reproduction and unreviewed scope for every finding. Lint and test success do not replace review.

For audio, trace the callback transitively through command application, scheduling, DSP, lifecycle and deferred reclamation. Check allocation/free/locks/I/O, bounded loops and capacities, saturation policy, numeric stability, denormals, sample-rate propagation and feature-family symmetry. Review unsafe/FFI assumptions and ownership/thread boundaries explicitly.

For the GUI, trace data acquisition, analysis, layout, geometry, uploads, gestures and edit-back. Check that work follows visible physical pixels where appropriate, shared Rust logic is implemented once, native/browser rules match, resources are released and partial data/connection failures recover. Inspect hidden idle redraws and large-data behavior rather than relying on FPS alone.

Retain fmt/clippy/feature-matrix evidence from isolated outputs. Consult the repository's relevant skills before writing any instrumented Rust/DSP/widget changes. No manual audio/visual behavior is claimed from headless tests. Attach evidence to the corresponding review step and record accepted defects or pending work in findings.md, preserving links to the owning plans when changes there are authorized.

Before reporting a cause as unresolved, search the repository's plans, `docs/decisions.md` and module docs for the symptom (a signal's name, an error string, a limit); a cause the project already documents is a finding about its scope. Before reporting an input as absent, search the tree for it by name and by the identifiers it should contain, and check whether any CI or release job runs the failing test at all. Rank severity by what was observed, how likely the path is in the project's real use and what a fix would change; an unreproduced structural concern whose fix changes semantics is a future direction, not a high-severity defect.
