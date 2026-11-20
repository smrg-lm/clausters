# Standalone editor audit

Treat clausters-editor as an application of its own, not a server client. Its crate (crates/clausters-editor), its page (crates/clausters-editor/page) and its Python wheel manifests all belong to the editor unit and need package_coverage entries. The editor's README and .github/workflows/clausters-editor.yml define its existing build and third-party license gates.

After isolated setup and automatic checks, review session process launch/interrupt/termination, Python environment selection, file save/recovery, editor commands, UI state, error handling, IPC permissions and external-link handling. Inspect the platform boundary (the session's process, the files, the dialogs) and, in the page, the boundary between the application's wasm and the page's script. Test with an ordinary Python environment without clausters installed as well as the intended integration; the editor must remain independent.

Run node scripts/notices.mjs --check in the crate with cargo-about 0.9.2 and retain the output/version. Do not regenerate notices to hide stale license records or change any dependency during the audit. Retain packaging evidence for the Python wheel and native runtime/system requirements separately from frontend build success.

For efficiency, select and hash a representative Python file plus a larger generated file, fix viewport/font/scale, warm up 10 seconds and collect three 60-second idle, typing/navigation and interactive-session trials. Record startup/readiness, response latency where measurable, CPU/RSS and retained raw samples. Save screenshots and an exact input timeline. Process CPU alone does not establish UI latency. Record unavailable metrics and actual UI behavior without extrapolating from headless checks.

Attach editor-review evidence, including package_coverage for the editor's Rust, frontend and wheel manifests; each entry has status, reason and retained evidence reference. Missing UI/session/license/package evidence leaves the campaign partial. No result is combined with the Python/web clients or the audio project's baseline.
