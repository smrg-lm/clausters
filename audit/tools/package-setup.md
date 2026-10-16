# Isolated package prerequisites

Prepare a separate external work root for each package unit. The harness runtime has no third-party dependencies; these are the packages' own test/build prerequisites, installed only in the disposable clone or external environment. No setup here changes the application checkout or dependency declarations. Setup is offline and records commands, outcomes and Python wheel hashes. Inspect failed receipts before preparing a fresh root; do not treat incomplete setup as success.

## Python reference client

Provision a wheelhouse explicitly before setup. The audit-only tools/python-requirements.txt pins pytest and its Linux dependencies; fetching is a separate operator action requiring network access. For example, use a disposable provisioning environment with pip to download those exact versions into /data/audit-wheelhouse. Retain and review the downloaded hashes; setup records them. Do not install into the maintainer's .venv.

```sh
python3 /tmp/clausters-python-audit/resources/scripts/audit.py setup-unit --work /tmp/clausters-python-audit --unit python --wheelhouse /data/audit-wheelhouse
python3 /tmp/clausters-python-audit/resources/scripts/audit.py run --work /tmp/clausters-python-audit --scenario /tmp/clausters-python-audit/resources/scenarios/python.json --name python-01
```

setup-unit creates tools/client-python with pip and installs the pinned wheels without network or dependency resolution, then checks consistency. The scenario stages complete native artifacts with the source's build_native.py in the isolated clone, sets CLAUSTERS_REQUIRE_COMPLETE=1 and runs pytest there. libfaust/LLVM, libverovio and native system headers remain explicit system prerequisites from that revision's build documentation. They are never installed globally by the runner. Prepared target symlinks point existing builders at the external target directory. The isolated source is on PYTHONPATH, so no editable installation of the main checkout is used.

## Web client

Provision the external tools/npm-cache from the selected source's package-lock.json with a separately authorized npm ci invocation using that cache. Also provision wasm32-unknown-unknown and the wasm-bindgen CLI matching the source and GUI lockfiles; install the CLI under the external tools/cargo prefix if needed, not into a project manifest. The source build.sh verifies the bindgen version. Record browser/system package versions and use one browser for ordinary development acceptance.

```sh
python3 /tmp/clausters-web-audit/resources/scripts/audit.py setup-unit --work /tmp/clausters-web-audit --unit web
python3 /tmp/clausters-web-audit/resources/scripts/audit.py run --work /tmp/clausters-web-audit --scenario /tmp/clausters-web-audit/resources/scenarios/web.json --name web-01
```

setup-unit runs npm ci --offline against the isolated source and external cache. The scenario builds native WS prerequisites, runs build.sh before test.sh, and records node/browser acceptance. Target symlinks make build.sh's literal workspace target paths refer to the external build directory. Missing dependencies or self-skipped tests cannot silently accept the package. Retain native binary locations and test logs so the WS paths are known to have executed.

## Standalone editor

Provision its own npm lockfile cache and the system WebKitGTK/Tauri headers listed in the selected editor README/workflow. Do not borrow the web client's node_modules. Its scenario checks the frontend, builds it, and runs fmt/clippy/tests in its independent Rust workspace. The optional license command uses cargo-about 0.9.2, matching the editor workflow; its output is mandatory manual review evidence.

```sh
python3 /tmp/clausters-editor-audit/resources/scripts/audit.py setup-unit --work /tmp/clausters-editor-audit --unit editor
python3 /tmp/clausters-editor-audit/resources/scripts/audit.py run --work /tmp/clausters-editor-audit --scenario /tmp/clausters-editor-audit/resources/scenarios/editor.json --name editor-01
```

No library is added, removed or updated by the audit. Use npm run notices -- --check; do not regenerate licenses to hide drift. If tests execute zero cases, the campaign records an error and requires a reviewed scenario revision or real test coverage; zero tests are not evidence of correctness.


Before the web build, explicitly provision `vendor/verovio/{verovio.js,verovio.wasm}` and `vendor/faust/{libfaust-wasm.js,libfaust-wasm.wasm,libfaust-wasm.data,version}` in the disposable source. Build them from the selected source's third_party pins or copy a reviewed cache with retained source/build receipts. The harness checks presence, nonempty bytes and hashes before a web build; it never downloads or rebuilds them implicitly. Generated declaration diffs are retained by web-build, so repeat runs do not mistake builder output for intentional instrumentation.
